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
use hayro::vello_cpu::color::{palette, AlphaColor, Srgb};
use hayro::vello_cpu::Pixmap;
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
    /// 1-based numbers of the listed pages that hold ONLY an interactive 3D
    /// model (see `model_3d`). Absent from the JSON for every other PDF, so the
    /// answer for an ordinary document is byte for byte what it was.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub model3d: Vec<usize>,
    /// For a file with 3D-only pages: the vault-relative path of its 2D drawing
    /// in the same folder, if there is one (`twin_of`). Filled by main.rs, which
    /// knows the vault; None everywhere else and then absent from the JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twin: Option<String>,
}

/* THE 2D DRAWING NEXT TO A 3D MODEL. A CAD export usually comes as a pair: the
   drawing sheets ("Chassis - Ind 00.pdf") and the model ("Chassis - Ind 00 -
   3D.PDF"). The model cannot be drawn here, the drawing can, so the frame of
   the model offers the drawing.
   The pairing is by NAME, and deliberately narrow, so it never offers an
   unrelated file: the 3D file's name must END in "3D" set apart by a space, a
   dash, an underscore or a dot ("X - 3D", "X 3D", "X_3D"); the twin is the
   name before that marker, trailing separators trimmed, compared without case,
   in the SAME folder, and a PDF. "Model3D.pdf" has no separate marker and gets
   no twin. */

/// the twin's stem for a 3D file's stem, or None if the name does not follow the pattern
pub fn twin_stem(stem: &str) -> Option<&str> {
    let t = stem.trim_end();
    let cut = t.len().checked_sub(2)?;
    if !t.is_char_boundary(cut) || !t[cut..].eq_ignore_ascii_case("3d") {
        return None;
    }
    let before = &t[..cut];
    if !before.ends_with([' ', '-', '_', '.']) {
        return None;
    }
    let base = before.trim_end_matches([' ', '-', '_', '.']);
    (!base.is_empty()).then_some(base)
}

/// among `names` (one folder's entries), the 2D drawing of the 3D file `name`.
/// Where case matters (Linux) a folder can hold "Part.pdf" and "part.PDF" side
/// by side; the answer must not depend on the order the folder is listed in, so
/// the stem written exactly as in the 3D file's name wins, then the first name
/// in byte order.
pub fn twin_of<'a>(name: &str, names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let (stem, _) = name.rsplit_once('.')?;
    let want = twin_stem(stem)?;
    names
        .into_iter()
        .filter_map(|n| {
            let (s, e) = n.rsplit_once('.')?;
            let s = s.trim_end();
            (n != name && e.eq_ignore_ascii_case(PDF_EXT) && s.eq_ignore_ascii_case(want)).then_some((s != want, n))
        })
        .min()
        .map(|(_, n)| n)
}

/* A "3D PDF" (CAD exports: SolidWorks, CATIA, ...) is a page with NO drawing of
   its own and one /3D annotation carrying the model as a PRC or U3D stream. A
   reader that implements those formats draws the model live; everything else —
   this renderer, and every browser's PDF viewer — draws the page's content,
   which is nothing, and shows a white sheet. A blank card looks like a bug, so
   such a page is NAMED: the frame then says what the file is and how to see it
   (ui/editor.js pdfFill). Decoding PRC is a 3D engine of its own and is not
   attempted.
   Only a page with an EMPTY content stream is reported. A page that draws
   something (a title block, or the poster image some exporters add) is rendered
   as usual: showing what is there beats replacing it with a notice.
   A /RichMedia annotation (PDF 1.7 ext. 3) also carries video, sound or Flash;
   it counts only when one of its configurations is /3D. */
fn model_3d(pg: &hayro::hayro_syntax::page::Page) -> bool {
    use hayro::hayro_syntax::object::{Array, Dict, Name};
    let is = |d: &Dict, t: &[u8]| d.get::<Name>("Subtype").is_some_and(|n| n.as_ref() == t);
    let rich_3d = |a: &Dict| {
        a.get::<Dict>("RichMediaContent")
            .and_then(|c| c.get::<Array>("Configurations"))
            .is_some_and(|cs| cs.iter::<Dict>().any(|c| is(&c, b"3D")))
    };
    let Some(annots) = pg.raw().get::<Array>("Annots") else { return false };
    let has_model = annots.iter::<Dict>().any(|a| is(&a, b"3D") || (is(&a, b"RichMedia") && rich_3d(&a)));
    has_model && pg.page_stream().is_none_or(|s| s.iter().all(u8::is_ascii_whitespace))
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
    let model3d = pages
        .iter()
        .take(MAX_LISTED)
        .enumerate()
        .filter(|(_, pg)| catch_unwind(AssertUnwindSafe(|| model_3d(pg))).unwrap_or(false))
        .map(|(i, _)| i + 1)
        .collect();
    Info { pages: pages.len(), sizes, model3d, twin: None }
}

/// the scale for page (pw x ph) pt rendered `w` px wide, within the caps
pub fn scale_for(pw: f32, ph: f32, w: u32) -> Option<f32> {
    if !dims_ok(pw, ph) {
        return None;
    }
    let s = (w.min(MAX_W) as f32 / pw).min(MAX_SIDE / pw).min(MAX_SIDE / ph);
    (s.is_finite() && s > 0.0).then_some(s)
}

/// issue #44: the paper. hayro's RenderSettings::default() has bg_color TRANSPARENT,
/// so a page that does not paint its own background (nearly every real PDF: text on
/// an unpainted page) came out as glyphs on alpha 0 — dark text on the dark theme.
/// A PDF page is white paper unless its content paints otherwise (as pdf.js/Poppler).
pub const PAPER: AlphaColor<Srgb> = palette::css::WHITE;

/// page N (1-based) rendered `w` px wide (capped) on opaque PAPER, or None. Never panics.
pub fn page_pixmap(pdf: &Pdf, page: usize, w: u32) -> Option<Pixmap> {
    let pg = pdf.pages().get(page.checked_sub(1)?)?;
    catch_unwind(AssertUnwindSafe(|| {
        let (pw, ph) = pg.render_dimensions();
        let s = scale_for(pw, ph, w)?;
        let cache = RenderCache::new();
        let rs = RenderSettings { x_scale: s, y_scale: s, bg_color: PAPER, ..Default::default() };
        Some(render(pg, &cache, &InterpreterSettings::default(), &rs))
    }))
    .ok()
    .flatten()
}

/// page N (1-based) as PNG `w` px wide (capped), or None. Never panics.
pub fn page_png(pdf: &Pdf, page: usize, w: u32) -> Option<Vec<u8>> {
    let px = page_pixmap(pdf, page, w)?;
    catch_unwind(AssertUnwindSafe(|| px.into_png().ok())).ok().flatten()
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

    /// a one-page PDF whose page dictionary gets `extra` (annotations, contents);
    /// object 4 is a content stream that draws a rectangle, object 5 a 3D annotation
    fn page_pdf(extra: &str) -> Vec<u8> {
        let stream = "0.5 g 0 0 100 100 re f\n";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 792 612] /Resources << >> {extra} >>"),
            format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len()),
            "<< /Type /Annot /Subtype /3D /Rect [50 50 750 550] /Contents (3D Model) >>".to_string(),
            "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".to_string(),
            // 7: rich media holding a video; 8: rich media holding a 3D model
            "<< /Type /Annot /Subtype /RichMedia /Rect [50 50 750 550] /RichMediaContent << /Configurations [<< /Type /RichMediaConfiguration /Subtype /Video >>] >> >>".to_string(),
            "<< /Type /Annot /Subtype /RichMedia /Rect [50 50 750 550] /RichMediaContent << /Configurations [<< /Type /RichMediaConfiguration /Subtype /Sound >> << /Type /RichMediaConfiguration /Subtype /3D >>] >> >>".to_string(),
        ];
        let mut out = b"%PDF-1.6\n".to_vec();
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

    /// a page that is nothing but a 3D model is named; a page that draws
    /// something is rendered as ever, 3D annotation or not; and an ordinary
    /// PDF's answer does not even carry the key
    #[test]
    fn pdf_info_names_pages_that_hold_only_a_3d_model() {
        let of = |name: &str, extra: &str| info(&load(&tmp(name, &page_pdf(extra))).expect("parses")).model3d;
        assert_eq!(of("m3-only.pdf", "/Annots [5 0 R]"), vec![1], "empty page + 3D annotation");
        assert_eq!(of("m3-drawn.pdf", "/Annots [5 0 R] /Contents 4 0 R"), Vec::<usize>::new(), "the page draws something: render it");
        assert_eq!(of("m3-link.pdf", "/Annots [6 0 R]"), Vec::<usize>::new(), "an empty page with a link is just an empty page");
        assert_eq!(of("m3-none.pdf", ""), Vec::<usize>::new());
        assert_eq!(of("m3-video.pdf", "/Annots [7 0 R]"), Vec::<usize>::new(), "rich media that is a video is not a 3D model");
        assert_eq!(of("m3-rich3d.pdf", "/Annots [8 0 R]"), vec![1], "rich media with a 3D configuration is");
        let plain = info(&load(&tmp("m3-plain.pdf", &mini_pdf(2))).unwrap());
        assert!(plain.model3d.is_empty());
        assert_eq!(serde_json::to_string(&plain).unwrap(), r#"{"pages":2,"sizes":[[612.0,792.0],[612.0,792.0]]}"#, "an ordinary PDF answers exactly as before");
        let m = info(&load(&tmp("m3-json.pdf", &page_pdf("/Annots [5 0 R]"))).unwrap());
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"pages":1,"sizes":[[792.0,612.0]],"model3d":[1]}"#);
    }

    /// a 3D export is paired with its drawing by name, narrowly: the user's real
    /// file names pair, and nothing without a separate "3D" marker does
    #[test]
    fn pdf_twin_pairs_a_3d_model_with_its_drawing_by_name_only() {
        let dir = [
            "00 - Chassis - Ind 00.pdf", "00 - Chassis - Ind 00 - 3D.PDF",
            "Arbre Potager - Active.pdf", "Arbre Potager - Active 3D.PDF",
            "07 - Serre.pdf", "07 - Serre - Ind 00 - 3D.PDF",
            "Part_3D.pdf", "Part.PDF", "Model3D.pdf", "Model.pdf", "Notes.md", "Box.pdf.md", "Lone - 3D.pdf",
        ];
        let t = |n: &str| twin_of(n, dir);
        assert_eq!(t("00 - Chassis - Ind 00 - 3D.PDF"), Some("00 - Chassis - Ind 00.pdf"));
        assert_eq!(t("Arbre Potager - Active 3D.PDF"), Some("Arbre Potager - Active.pdf"));
        assert_eq!(t("Part_3D.pdf"), Some("Part.PDF"), "case of the extension does not matter");
        assert_eq!(t("07 - Serre - Ind 00 - 3D.PDF"), None, "07 - Serre.pdf is a different name: no guess");
        assert_eq!(t("Model3D.pdf"), None, "no separate marker: no twin");
        assert_eq!(t("Lone - 3D.pdf"), None, "no drawing in the folder");
        assert_eq!(t("00 - Chassis - Ind 00.pdf"), None, "a drawing has no twin");
        // where case matters both may sit side by side: the same answer in any listing order
        let both = ["part.PDF", "Part.pdf", "Part - 3D.pdf", "Part.pdf.bak"];
        let mut rev = both;
        rev.reverse();
        assert_eq!(twin_of("Part - 3D.pdf", both), Some("Part.pdf"), "the stem as written in the 3D name wins");
        assert_eq!(twin_of("Part - 3D.pdf", rev), Some("Part.pdf"));
        assert_eq!(twin_of("PART - 3D.pdf", both), Some("Part.pdf"), "no exact stem: first in byte order");
        assert_eq!(twin_of("PART - 3D.pdf", rev), Some("Part.pdf"));
        assert_eq!(twin_stem("X - 3d"), Some("X"));
        assert_eq!(twin_stem(" - 3D"), None);
        assert_eq!(twin_stem("3D"), None);
        assert_eq!(twin_stem("é 3D"), Some("é"));
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

    /// a one-page 200x100 PDF whose content does NOT paint the page background:
    /// a std-14 Helvetica line + a small black box — what real documents look like
    /// (mini_pdf's full-page fill is what hid #44 from every earlier test).
    fn unpainted_page_pdf() -> Vec<u8> {
        let content = "BT /F1 18 Tf 20 60 Td (PAPER 44) Tj ET 0 g 150 20 20 20 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
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

    #[test]
    fn pdf_unpainted_page_renders_on_opaque_white_paper() {
        // issue #44: hayro's default background is TRANSPARENT; on the dark theme a
        // text-only page was dark glyphs on alpha 0 — the user saw "no PDF".
        let p = tmp("unpainted.pdf", &unpainted_page_pdf());
        let d = load(&p).expect("unpainted pdf parses");
        let px = page_pixmap(&d, 1, 200).expect("page 1 renders");
        assert_eq!((px.width(), px.height()), (200, 100));
        // the corners hold no content: they must be the paper, fully opaque white
        for (x, y) in [(0, 0), (199, 0), (0, 99), (199, 99), (100, 5)] {
            let c = px.sample(x, y);
            assert_eq!((c.r, c.g, c.b, c.a), (255, 255, 255, 255), "paper at ({x},{y})");
        }
        // EVERY pixel is opaque: nothing of the page lets the dark theme through
        assert!(px.data().iter().all(|c| c.a == 255), "a page pixel is not opaque");
        // and the content is still there: the box (y flipped: 150..170 x, 80..60 px y)
        let b = px.sample(160, 70);
        assert_eq!((b.r, b.g, b.b, b.a), (0, 0, 0, 255), "black box");
        // the text line drew dark ink in its band (x 20..130, y ~22..40 px)
        let ink = (22..42u16).flat_map(|y| (20..130u16).map(move |x| (x, y)))
            .filter(|&(x, y)| px.sample(x, y).r < 128).count();
        assert!(ink > 50, "the text drew {ink} dark pixels");
        // the PNG the webview gets carries the same paper (RGBA, not transparent)
        let png = page_png(&d, 1, 200).expect("png");
        assert_eq!(&png[1..4], b"PNG");
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
