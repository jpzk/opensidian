// SPDX-License-Identifier: GPL-3.0-or-later
/* pdfembed — PDF embeds (`![[f.pdf]]`), rasterised HERE, never in the webview.
   The design and its reasons are harness docs/recon-pdfembed/README.md
   ("security design"); this file is that section, executable:

   - Renderer: hayro =0.7.1 (Apache-2.0 OR MIT, pure Rust). It has no
     JavaScript engine, no form/XFA engine, no action dispatcher and no I/O
     of its own: it turns page content streams into pixels and nothing else,
     so /JavaScript, /OpenAction, /Launch and /URI in a hostile PDF are bytes
     it never acts on. The webview only ever sees a PNG.
   - Serving: the EXISTING `opensidian-img` scheme, `<path>.pdf?page=N&w=PX`
     -> image/png. No new scheme, no CSP change, no asset scope. Containment
     is main.rs `pdf_path_in` (the R29.5 rule). A `.pdf` without a valid
     query serves nothing — the raw PDF bytes never reach the webview, so
     WebKit's own PDF viewer can never be handed one.
   - Bounds: file <= MAX_PDF_BYTES; output width <= MAX_W and neither side
     > MAX_SIDE px; ONE worker thread renders, FIFO, so a hostile page costs
     one core, never the UI thread, and N embeds never cost N x memory;
     catch_unwind around every parse and render; bounded doc + png caches. */

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::SystemTime;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::{render, RenderCache, RenderSettings};

pub const PDF_EXT: &str = "pdf";
/// README: a bigger file is not read at all (placeholder instead)
pub const MAX_PDF_BYTES: u64 = 256 * 1024 * 1024;
/// requested width is clamped to this many px
pub const MAX_W: u32 = 2048;
/// ...and the page is scaled so neither side exceeds this (a page that
/// claims 10^6 pt must not buy a gigapixel pixmap)
pub const MAX_SIDE: f32 = 4096.0;
/// `pdf_info` lists at most this many page sizes (the UI says "first N shown")
pub const MAX_LISTED: usize = 2000;
/// a page number above this is not a request we parse
const MAX_PAGE_NO: usize = 1_000_000;
const DOC_CACHE: usize = 4;
const PNG_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// what `pdf_info` answers: total page count + the first MAX_LISTED page
/// sizes in PDF points (w, h), as displayed (rotation applied).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Info {
    pub pages: usize,
    pub sizes: Vec<[f32; 2]>,
}

/// `?page=N&w=PX` -> (N, PX), strictly: both keys present, decimal, N >= 1,
/// PX >= 1 (clamped to MAX_W). Anything else -> None (404).
pub fn parse_query(q: Option<&str>) -> Option<(usize, u32)> {
    let (mut page, mut w) = (None, None);
    for kv in q?.split('&') {
        let (k, v) = kv.split_once('=')?;
        if v.is_empty() || v.len() > 7 || !v.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: usize = v.parse().ok()?;
        match k {
            "page" if page.is_none() => page = Some(n),
            "w" if w.is_none() => w = Some(n),
            _ => return None,
        }
    }
    let (page, w) = (page?, w?);
    if page == 0 || page > MAX_PAGE_NO || w == 0 {
        return None;
    }
    Some((page, (w as u64).min(MAX_W as u64) as u32))
}

/// read (capped, re-checked on the stream) and parse. None for oversize,
/// unreadable, unparseable — and for a parser panic.
pub fn load(p: &Path) -> Option<Pdf> {
    use std::io::Read;
    let f = std::fs::File::open(p).ok()?;
    let mut bytes = Vec::new();
    f.take(MAX_PDF_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_PDF_BYTES {
        return None;
    }
    catch_unwind(AssertUnwindSafe(|| Pdf::new(bytes).ok())).ok().flatten()
}

fn dims_ok(w: f32, h: f32) -> bool {
    w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
}

pub fn info(pdf: &Pdf) -> Info {
    let pages = pdf.pages();
    let sizes = pages
        .iter()
        .take(MAX_LISTED)
        .map(|pg| {
            let (w, h) = catch_unwind(AssertUnwindSafe(|| pg.render_dimensions())).unwrap_or((0.0, 0.0));
            if dims_ok(w, h) { [w, h] } else { [612.0, 792.0] }
        })
        .collect();
    Info { pages: pages.len(), sizes }
}

/// the scale for page (pw x ph) pt rendered `w` px wide, within the caps
pub fn scale_for(pw: f32, ph: f32, w: u32) -> Option<f32> {
    if !dims_ok(pw, ph) {
        return None;
    }
    let s = (w.min(MAX_W) as f32 / pw).min(MAX_SIDE / pw).min(MAX_SIDE / ph);
    (s.is_finite() && s > 0.0).then_some(s)
}

/// page N (1-based) as PNG `w` px wide (capped), or None. Never panics.
pub fn page_png(pdf: &Pdf, page: usize, w: u32) -> Option<Vec<u8>> {
    let pg = pdf.pages().get(page.checked_sub(1)?)?;
    catch_unwind(AssertUnwindSafe(|| {
        let (pw, ph) = pg.render_dimensions();
        let s = scale_for(pw, ph, w)?;
        let cache = RenderCache::new();
        let rs = RenderSettings { x_scale: s, y_scale: s, ..Default::default() };
        render(pg, &cache, &InterpreterSettings::default(), &rs).into_png().ok()
    }))
    .ok()
    .flatten()
}

/* ---- the ONE render worker -------------------------------------------- */

pub type Reply<T> = Box<dyn FnOnce(Option<T>) + Send>;

enum Job {
    Info(PathBuf, Reply<Info>),
    Render(PathBuf, usize, u32, Reply<Arc<Vec<u8>>>),
}

/// a cache key that goes stale when the file changes on disk
type Key = (PathBuf, Option<SystemTime>, u64);

fn key_of(p: &Path) -> Option<Key> {
    let m = std::fs::symlink_metadata(p).ok()?;
    Some((p.to_path_buf(), m.modified().ok(), m.len()))
}

#[derive(Default)]
struct Worker {
    docs: VecDeque<(Key, Option<Pdf>)>,           // MRU first; None = known-bad
    pngs: VecDeque<((Key, usize, u32), Arc<Vec<u8>>)>,
    png_bytes: usize,
}

impl Worker {
    fn doc(&mut self, k: &Key) -> Option<&Pdf> {
        if let Some(i) = self.docs.iter().position(|(dk, _)| dk == k) {
            let e = self.docs.remove(i).expect("index in range");
            self.docs.push_front(e);
        } else {
            let d = load(&k.0);
            self.docs.push_front((k.clone(), d));
            self.docs.truncate(DOC_CACHE);
        }
        self.docs.front().and_then(|(_, d)| d.as_ref())
    }

    fn render(&mut self, p: &Path, page: usize, w: u32) -> Option<Arc<Vec<u8>>> {
        let k = key_of(p)?;
        let pk = (k.clone(), page, w);
        if let Some((_, b)) = self.pngs.iter().find(|(x, _)| *x == pk) {
            return Some(b.clone());
        }
        let png = Arc::new(page_png(self.doc(&k)?, page, w)?);
        if png.len() <= PNG_CACHE_BYTES {
            self.png_bytes += png.len();
            self.pngs.push_front((pk, png.clone()));
            while self.png_bytes > PNG_CACHE_BYTES {
                let Some((_, old)) = self.pngs.pop_back() else { break };
                self.png_bytes -= old.len();
            }
        }
        Some(png)
    }

    fn run(mut self, rx: mpsc::Receiver<Job>) {
        for job in rx {
            match job {
                Job::Info(p, reply) => {
                    let r = key_of(&p).and_then(|k| self.doc(&k).map(info));
                    reply(r)
                }
                Job::Render(p, page, w, reply) => {
                    let r = self.render(&p, page, w);
                    reply(r)
                }
            }
        }
    }
}

fn worker() -> Option<&'static mpsc::Sender<Job>> {
    static TX: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("pdf-render".into())
            .spawn(move || Worker::default().run(rx))
            .ok()
            .map(|_| tx)
    })
    .as_ref()
}

/// queue an info request for an ALREADY-CONTAINED path (main.rs pdf_path_in)
pub fn queue_info(p: PathBuf, reply: Reply<Info>) {
    match worker() {
        Some(tx) => {
            if let Err(mpsc::SendError(Job::Info(_, r))) = tx.send(Job::Info(p, reply)) {
                r(None)
            }
        }
        None => reply(None),
    }
}

/// queue a page render for an ALREADY-CONTAINED path (main.rs pdf_path_in)
pub fn queue_render(p: PathBuf, page: usize, w: u32, reply: Reply<Arc<Vec<u8>>>) {
    match worker() {
        Some(tx) => {
            if let Err(mpsc::SendError(Job::Render(_, _, _, r))) = tx.send(Job::Render(p, page, w, reply)) {
                r(None)
            }
        }
        None => reply(None),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// a REAL, minimal, n-page PDF with an exact xref (no fallback parsing),
    /// page i is 612x792 with a filled rect whose gray level encodes i — so a
    /// test can tell page 2 from page 1 by a pixel, like the smoke palette.
    pub fn mini_pdf(n: usize) -> Vec<u8> {
        let mut objs: Vec<String> = Vec::new();
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
        objs.push("<< /Type /Catalog /Pages 2 0 R >>".into());
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
        for i in 0..n {
            let g = (i + 1) as f32 / (n + 1) as f32;
            let content = format!("{g} g 0 0 612 792 re f");
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R >>",
                4 + 2 * i
            ));
            objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
        for o in offs {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
        out
    }

    fn tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("opensidian-pdf-{}-{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn pdf_query_is_strict() {
        assert_eq!(parse_query(Some("page=3&w=700")), Some((3, 700)));
        assert_eq!(parse_query(Some("w=700&page=1")), Some((1, 700)));
        assert_eq!(parse_query(Some("page=1&w=999999")), Some((1, MAX_W)));
        for q in ["", "page=1", "w=700", "page=0&w=700", "page=1&w=0", "page=-1&w=7",
                  "page=1&w=7&x=1", "page=1&page=2&w=7", "page=1e3&w=7", "page=&w=7",
                  "page=1&w=7&", "page=99999999&w=7", "page=%31&w=7"] {
            assert_eq!(parse_query(Some(q)), None, "{q:?}");
        }
        assert_eq!(parse_query(None), None);
    }

    #[test]
    fn pdf_scale_caps_both_sides() {
        assert_eq!(scale_for(612.0, 792.0, 612), Some(1.0));
        let s = scale_for(612.0, 792.0, 100_000).unwrap();
        assert!((612.0 * s).round() <= MAX_W as f32);
        // a page claiming 10^6 pt tall must not buy a gigapixel pixmap
        let s = scale_for(100.0, 1_000_000.0, 2048).unwrap();
        assert!(100.0 * s <= MAX_SIDE && 1_000_000.0 * s <= MAX_SIDE + 0.5);
        for (w, h) in [(0.0, 792.0), (612.0, -1.0), (f32::NAN, 1.0), (f32::INFINITY, 1.0)] {
            assert_eq!(scale_for(w, h, 700), None);
        }
    }

    #[test]
    fn pdf_info_and_pages_render_the_requested_page() {
        let p = tmp("three.pdf", &mini_pdf(3));
        let d = load(&p).expect("mini pdf parses");
        let i = info(&d);
        assert_eq!(i.pages, 3);
        assert_eq!(i.sizes, vec![[612.0, 792.0]; 3]);
        let png = page_png(&d, 2, 306).expect("page 2 renders");
        assert_eq!(&png[1..4], b"PNG");
        // IHDR width/height: 306 x 396 (half scale)
        let wh = |b: &[u8], o: usize| u32::from_be_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!((wh(&png, 16), wh(&png, 20)), (306, 396));
        // pages differ (each page has its own gray) and page 4 does not exist
        assert_ne!(page_png(&d, 1, 50), page_png(&d, 3, 50));
        assert_eq!(page_png(&d, 4, 50), None);
        assert_eq!(page_png(&d, 0, 50), None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn pdf_broken_and_oversize_load_nothing_and_never_panic() {
        let p = tmp("broken.pdf", b"%PDF-1.4\n1 0 obj << /Type /Catalog");
        assert!(load(&p).is_none());
        std::fs::write(&p, b"").unwrap();
        assert!(load(&p).is_none());
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_len(MAX_PDF_BYTES + 1).unwrap();
        assert!(load(&p).is_none(), "over the cap is never parsed");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn pdf_worker_serves_fifo_and_caches() {
        let p = tmp("worker.pdf", &mini_pdf(2));
        let (tx, rx) = mpsc::channel();
        let t2 = tx.clone();
        queue_info(p.clone(), Box::new(move |r| tx.send(format!("{:?}", r.map(|i| i.pages))).unwrap()));
        queue_render(p.clone(), 2, 100, Box::new(move |r| t2.send(format!("{}", r.map(|b| b.len() > 8).unwrap_or(false))).unwrap()));
        assert_eq!(rx.recv().unwrap(), "Some(2)");
        assert_eq!(rx.recv().unwrap(), "true");
        let (tx, rx) = mpsc::channel();
        queue_render(p.clone(), 9, 100, Box::new(move |r| tx.send(r.is_none()).unwrap()));
        assert!(rx.recv().unwrap(), "a page that does not exist renders nothing");
        let _ = std::fs::remove_file(&p);
    }
}
