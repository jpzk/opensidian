// SPDX-License-Identifier: GPL-3.0-or-later
/* #197: a saved recording gets its length.

   MediaRecorder writes WebM as a LIVE stream: the muxer cannot know the length up
   front, so the Segment Info carries no Duration (the Matroska spec, RFC 9559
   §5.1.2.3, makes Info/Duration optional; a live Segment may also have an unknown
   size, §6.2). A player then sees an unknown duration: WebKit's media controls keep
   their activity indicator and show "-:--" (jpzk/opensidian#197).

   At save time the whole file is known, so we write the Duration the muxer could
   not: the end of the last block (Cluster Timestamp + the block's relative
   timestamp, §10.1/§10.2, plus the last gap between two blocks of that track as the
   last frame's length), in TimestampScale units (default 1 000 000 ns, §5.1.2.1),
   as an 8-byte float element 0x4489 inside Info 0x1549A966.

   Conservative by design: anything we do not fully understand leaves the bytes
   as they are (None). An existing Duration of 0 / non-finite (a muxer that left a
   placeholder) is overwritten in place, same size. A file with a SeekHead or Cues
   (absolute positions inside the Segment) and no Duration is left alone: growing
   Info would shift what they point at. */

const ID_EBML: u32 = 0x1A45_DFA3;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_VOID: u32 = 0xEC;
const ID_SEEKHEAD: u32 = 0x114D_9B74;
const ID_INFO: u32 = 0x1549_A966;
const ID_TSSCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_CUES: u32 = 0x1C53_BB6B;
const ID_TIMESTAMP: u32 = 0xE7;
const ID_SIMPLEBLOCK: u32 = 0xA3;
const ID_BLOCKGROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
/// Segment children that end an unknown-size Cluster (§6.2: an unknown-size element
/// ends at the first element that is not a valid child)
const TOP: [u32; 8] = [ID_CLUSTER, ID_CUES, ID_SEEKHEAD, ID_INFO, 0x1654_AE6B, 0x1043_A770, 0x1941_A469, 0x1254_C367];

/// an element ID (marker bits kept, 1..4 bytes) at `p` -> (id, length)
fn read_id(b: &[u8], p: usize) -> Option<(u32, usize)> {
    let f = *b.get(p)?;
    let n = f.leading_zeros() as usize + 1;
    if n > 4 || p + n > b.len() {
        return None;
    }
    Some((b[p..p + n].iter().fold(0u32, |a, &x| (a << 8) | x as u32), n))
}

/// a variable-size integer at `p` -> (value, length); value None = "unknown" (all ones)
fn read_vint(b: &[u8], p: usize) -> Option<(Option<u64>, usize)> {
    let f = *b.get(p)?;
    let n = f.leading_zeros() as usize + 1;
    if n > 8 || p + n > b.len() {
        return None;
    }
    let mut v = (f as u64) & (0xFF >> n);
    for &x in &b[p + 1..p + n] {
        v = (v << 8) | x as u64;
    }
    let ones = (1u64 << (7 * n)) - 1;
    Some((if v == ones { None } else { Some(v) }, n))
}

/// an element header at `p` -> (id, data start, data size or None = unknown)
fn header(b: &[u8], p: usize) -> Option<(u32, usize, Option<u64>)> {
    let (id, il) = read_id(b, p)?;
    let (sz, sl) = read_vint(b, p + il)?;
    Some((id, p + il + sl, sz))
}

fn uint(b: &[u8]) -> Option<u64> {
    if b.is_empty() || b.len() > 8 {
        return None;
    }
    Some(b.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
}

/// a known size as an 8-byte vint (0x01 + 7 bytes)
fn size8(v: u64) -> [u8; 8] {
    let mut o = v.to_be_bytes();
    o[0] = 0x01;
    o
}

/// per-track block timestamps -> the end of the last frame (TimestampScale units)
#[derive(Default)]
struct Ends {
    /// (track, last block start, last positive gap between two of its blocks)
    t: Vec<(u64, i64, i64)>,
}
impl Ends {
    fn block(&mut self, data: &[u8], cluster: i64) -> Option<()> {
        let (track, tl) = read_vint(data, 0)?;
        let track = track?;
        let rel = i16::from_be_bytes([*data.get(tl)?, *data.get(tl + 1)?]) as i64;
        let ts = cluster + rel;
        match self.t.iter_mut().find(|e| e.0 == track) {
            Some(e) => {
                if ts > e.1 {
                    e.2 = ts - e.1;
                    e.1 = ts;
                }
            }
            None => self.t.push((track, ts, 0)),
        }
        Some(())
    }
    fn end(&self) -> Option<i64> {
        self.t.iter().map(|e| e.1 + e.2).max()
    }
}

/// the byte edits that give the recording its Duration (offsets into the ORIGINAL bytes)
enum Edit {
    /// overwrite a 0 / non-finite Duration placeholder in place (same size)
    Put(usize, Vec<u8>),
    /// replace Info [at, end) with `info` and, for a known-size Segment, its size field [at, end) with `seg`
    Grow { info: (usize, usize, Vec<u8>), seg: Option<(usize, usize, [u8; 8])> },
}

/// the recording with a Duration in its Segment Info, or None = leave the bytes as they are
#[cfg(test)]
pub fn with_duration(b: &[u8]) -> Option<Vec<u8>> {
    let mut o = b.to_vec();
    add_duration(&mut o).then_some(o)
}

/// #197 in place (codeant Major on #226: no second full-size buffer next to the
/// decoded upload near the 128 MiB cap): the Duration goes into `v` itself; the
/// growth is a few bytes inside Info (Vec::splice; a large block grows by realloc).
/// false = `v` is untouched (nothing to fix, or a layout we do not fully understand)
pub fn add_duration(v: &mut Vec<u8>) -> bool {
    let Some(ed) = plan(v) else { return false };
    match ed {
        Edit::Put(at, x) => v[at..at + x.len()].copy_from_slice(&x),
        Edit::Grow { info: (ia, ie, info), seg } => {
            v.reserve_exact(info.len().saturating_sub(ie - ia) + 8);
            v.splice(ia..ie, info); // Info lies after the Segment header: patch it first, the size field's offsets stay valid
            if let Some((sa, se, s)) = seg {
                v.splice(sa..se, s);
            }
        }
    }
    true
}

fn plan(b: &[u8]) -> Option<Edit> {
    // EBML header(s): WebKit's recorder repeats the EBML header before the Segment; Void (0xEC) is skipped too
    let (id, _, _) = header(b, 0)?;
    if id != ID_EBML {
        return None;
    }
    let mut seg_at = 0usize;
    let (seg_data, seg_size) = loop {
        let (id, d, sz) = header(b, seg_at)?;
        match id {
            ID_EBML | ID_VOID => seg_at = d.checked_add(usize::try_from(sz?).ok()?)?,
            ID_SEGMENT => break (d, sz),
            _ => return None,
        }
    };
    let seg_end = match seg_size {
        Some(s) => seg_data.checked_add(usize::try_from(s).ok()?)?.min(b.len()),
        None => b.len(),
    };
    let mut info: Option<(usize, usize, usize)> = None; // (element start, data start, data end)
    let mut dur_at: Option<(usize, usize)> = None; // (data start, data len) of an existing Duration
    let mut positions = false;
    let mut ends = Ends::default();
    let mut p = seg_data;
    while p < seg_end {
        let Some((id, d, sz)) = header(b, p) else { break }; // a cut tail: keep what we have
        match id {
            ID_CLUSTER => {
                let c_end = match sz {
                    Some(s) => d.checked_add(usize::try_from(s).ok()?)?.min(seg_end),
                    None => seg_end,
                };
                let mut cts: Option<i64> = None;
                let mut q = d;
                while q < c_end {
                    let Some((cid, cd, csz)) = header(b, q) else { break };
                    if sz.is_none() && TOP.contains(&cid) {
                        break; // the unknown-size Cluster ends here
                    }
                    let Some(csz) = csz else { return None };
                    let ce = cd.checked_add(usize::try_from(csz).ok()?)?;
                    if ce > b.len() {
                        break; // the last block was cut: ignore it
                    }
                    match cid {
                        ID_TIMESTAMP => cts = Some(i64::try_from(uint(&b[cd..ce])?).ok()?),
                        ID_SIMPLEBLOCK => ends.block(&b[cd..ce], cts?)?,
                        ID_BLOCKGROUP => {
                            let mut r = cd;
                            while r < ce {
                                let (gid, gd, gsz) = header(b, r)?;
                                let ge = gd.checked_add(usize::try_from(gsz?).ok()?)?;
                                if ge > ce {
                                    return None;
                                }
                                if gid == ID_BLOCK {
                                    ends.block(&b[gd..ge], cts?)?;
                                }
                                r = ge;
                            }
                        }
                        _ => {}
                    }
                    q = ce;
                }
                p = q.max(d);
                if sz.is_some() {
                    p = c_end;
                }
                continue;
            }
            ID_SEEKHEAD | ID_CUES => positions = true,
            ID_INFO => {
                let e = d.checked_add(usize::try_from(sz?).ok()?)?;
                if e > b.len() || info.is_some() {
                    return None;
                }
                let mut q = d;
                while q < e {
                    let (iid, idd, isz) = header(b, q)?;
                    let ie = idd.checked_add(usize::try_from(isz?).ok()?)?;
                    if ie > e {
                        return None;
                    }
                    if iid == ID_DURATION {
                        dur_at = Some((idd, ie - idd));
                    }
                    if iid == ID_TSSCALE && uint(&b[idd..ie])? == 0 {
                        return None;
                    }
                    q = ie;
                }
                info = Some((p, d, e));
            }
            _ => {}
        }
        let Some(sz) = sz else { return None }; // an unknown-size element we cannot walk
        p = d.checked_add(usize::try_from(sz).ok()?)?;
    }
    let (i_at, i_data, i_end) = info?;
    let end = ends.end().filter(|&e| e > 0)? as f64;
    if let Some((at, n)) = dur_at {
        let cur = match n {
            4 => f32::from_be_bytes(b[at..at + 4].try_into().ok()?) as f64,
            8 => f64::from_be_bytes(b[at..at + 8].try_into().ok()?),
            _ => return None,
        };
        if cur.is_finite() && cur > 0.0 {
            return None; // the muxer wrote a real length
        }
        return Some(Edit::Put(at, if n == 4 { (end as f32).to_be_bytes().to_vec() } else { end.to_be_bytes().to_vec() }));
    }
    if positions {
        return None;
    }
    // Info grows by the Duration element; its header is rewritten with an 8-byte size
    let mut body = b[i_data..i_end].to_vec();
    body.extend_from_slice(&[0x44, 0x89, 0x88]);
    body.extend_from_slice(&end.to_be_bytes());
    let mut new_info = b[i_at..i_at + 4].to_vec(); // the 4-byte Info ID
    new_info.extend_from_slice(&size8(body.len() as u64));
    new_info.extend_from_slice(&body);
    let grow = new_info.len() as u64 - (i_end - i_at) as u64;
    let (_, sid_len) = read_id(b, seg_at)?;
    // a known size counts the Segment's data only: + the grown Info (its own header change is outside);
    // an unknown size stays unknown
    let seg = match seg_size {
        None => None,
        Some(s) => Some((seg_at + sid_len, seg_data, size8(s.checked_add(grow)?))),
    };
    Some(Edit::Grow { info: (i_at, i_end, new_info), seg })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut o = id.to_vec();
        o.extend_from_slice(&size8(body.len() as u64));
        o.extend_from_slice(body);
        o
    }
    fn unk(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut o = id.to_vec();
        o.push(0x01);
        o.extend_from_slice(&[0xFF; 7]);
        o.extend_from_slice(body);
        o
    }
    fn sblock(track: u8, rel: i16) -> Vec<u8> {
        let mut d = vec![0x80 | track];
        d.extend_from_slice(&rel.to_be_bytes());
        d.extend_from_slice(&[0x80, 0xAA, 0xBB]); // flags + payload
        el(&[0xA3], &d)
    }
    fn cluster(ts: u16, rels: &[i16], unknown: bool) -> Vec<u8> {
        let mut c = el(&[0xE7], &ts.to_be_bytes());
        for &r in rels {
            c.extend(sblock(1, r));
        }
        if unknown { unk(&[0x1F, 0x43, 0xB6, 0x75], &c) } else { el(&[0x1F, 0x43, 0xB6, 0x75], &c) }
    }
    fn ebml() -> Vec<u8> {
        el(&[0x1A, 0x45, 0xDF, 0xA3], &el(&[0x42, 0x82], b"webm"))
    }
    fn info(extra: &[u8]) -> Vec<u8> {
        let mut i = el(&[0x2A, 0xD7, 0xB1], &[0x0F, 0x42, 0x40]); // 1 000 000 ns
        i.extend_from_slice(extra);
        el(&[0x15, 0x49, 0xA9, 0x66], &i)
    }
    fn tracks() -> Vec<u8> {
        el(&[0x16, 0x54, 0xAE, 0x6B], &el(&[0xAE], &el(&[0xD7], &[1])))
    }
    /// the Duration a file carries (walks the same way a player would)
    fn duration_of(b: &[u8]) -> Option<f64> {
        let (_, d, sz) = header(b, 0)?;
        let (_, sd, _) = header(b, d + sz? as usize)?;
        let (id, idd, isz) = header(b, sd)?;
        assert_eq!(id, ID_INFO);
        let mut q = idd;
        while q < idd + isz? as usize {
            let (iid, x, s) = header(b, q)?;
            if iid == ID_DURATION {
                return Some(match s? {
                    4 => f32::from_be_bytes(b[x..x + 4].try_into().unwrap()) as f64,
                    _ => f64::from_be_bytes(b[x..x + 8].try_into().unwrap()),
                });
            }
            q = x + s? as usize;
        }
        None
    }

    #[test]
    fn webmdur_live_recording_gets_its_length() {
        // what MediaRecorder writes: unknown-size Segment, unknown-size Clusters, 20 ms opus blocks, no Duration
        let mut seg = info(&[]);
        seg.extend(tracks());
        seg.extend(cluster(0, &[0, 20, 40, 60], true));
        seg.extend(cluster(1000, &[0, 20, 40], true));
        let mut f = ebml();
        f.extend(unk(&[0x18, 0x53, 0x80, 0x67], &seg));
        assert_eq!(duration_of(&f), None);
        let o = with_duration(&f).expect("patched");
        assert_eq!(duration_of(&o), Some(1060.0), "last block 1040 + its 20 ms frame");
        // everything after Info is byte-identical (no positions to fix)
        let tail = &f[f.len() - (seg.len() - info(&[]).len())..];
        assert!(o.ends_with(tail));
        // idempotent: a file with a real Duration is left alone
        assert!(with_duration(&o).is_none());
    }

    #[test]
    fn webmdur_webkit_layout_repeated_ebml_header_tags_first() {
        // the layout WebKit's recorder wrote on the box (develop 8e9199d, AR-197 head dump): the EBML header twice,
        // an unknown-size Segment, Tags before Info, then Tracks + unknown-size Clusters
        let mut seg = el(&[0x12, 0x54, 0xC3, 0x67], &el(&[0x73, 0x73], b"tag"));
        seg.extend(info(&el(&[0x4D, 0x80], b"GStreamer matroskamux")));
        seg.extend(tracks());
        seg.extend(cluster(0, &[0, 20, 40], true));
        seg.extend(cluster(60, &[0, 20, 40], true));
        let mut f = ebml();
        f.extend(ebml());
        f.extend(unk(&[0x18, 0x53, 0x80, 0x67], &seg));
        let o = with_duration(&f).expect("patched");
        let (_, d, sz) = header(&o, 0).unwrap();
        let (_, d2, sz2) = header(&o, d + sz.unwrap() as usize).unwrap();
        let (id, sd, _) = header(&o, d2 + sz2.unwrap() as usize).unwrap();
        assert_eq!(id, ID_SEGMENT);
        let (tid, td, tsz) = header(&o, sd).unwrap();
        assert_eq!(tid, 0x1254_C367);
        let (iid, idd, isz) = header(&o, td + tsz.unwrap() as usize).unwrap();
        assert_eq!(iid, ID_INFO);
        let info_body = &o[idd..idd + isz.unwrap() as usize];
        assert!(info_body.windows(11).any(|w| w[..3] == [0x44, 0x89, 0x88] && f64::from_be_bytes(w[3..].try_into().unwrap()) == 120.0));
        assert_eq!(o.len(), f.len() + 11, "Info grew by the 11-byte Duration (its size was already 8 bytes wide)");
    }

    #[test]
    fn webmdur_known_segment_size_grows_with_info() {
        let mut seg = info(&[]);
        seg.extend(tracks());
        seg.extend(cluster(0, &[0, 20, 40], false));
        let mut f = ebml();
        f.extend(el(&[0x18, 0x53, 0x80, 0x67], &seg));
        let o = with_duration(&f).unwrap();
        assert_eq!(duration_of(&o), Some(60.0));
        let (_, d, sz) = header(&o, 0).unwrap();
        let (_, sd, ssz) = header(&o, d + sz.unwrap() as usize).unwrap();
        assert_eq!(sd + ssz.unwrap() as usize, o.len(), "Segment size covers the grown Info exactly");
    }

    #[test]
    fn webmdur_placeholder_overwritten_in_place_real_one_kept() {
        let mut seg = info(&[0x44, 0x89, 0x84, 0, 0, 0, 0]); // Duration 0.0 (f32)
        seg.extend(el(&[0x11, 0x4D, 0x9B, 0x74], &[])); // a SeekHead: in place is still fine
        seg.extend(cluster(0, &[0, 20], true));
        let mut f = ebml();
        f.extend(unk(&[0x18, 0x53, 0x80, 0x67], &seg));
        let o = with_duration(&f).unwrap();
        assert_eq!(o.len(), f.len());
        assert_eq!(duration_of(&o), Some(40.0));
        let mut real = info(&[0x44, 0x89, 0x84]);
        real.extend_from_slice(&1234.0f32.to_be_bytes());
        let mut g = ebml();
        let mut s2 = real;
        s2.extend(cluster(0, &[0, 20], true));
        g.extend(unk(&[0x18, 0x53, 0x80, 0x67], &s2));
        assert!(with_duration(&g).is_none());
    }

    #[test]
    fn webmdur_leaves_what_it_cannot_patch_safely() {
        // SeekHead + no Duration: growing Info would shift positions -> untouched
        let mut seg = el(&[0x11, 0x4D, 0x9B, 0x74], &[]);
        seg.extend(info(&[]));
        seg.extend(cluster(0, &[0, 20], true));
        let mut f = ebml();
        f.extend(unk(&[0x18, 0x53, 0x80, 0x67], &seg));
        assert!(with_duration(&f).is_none());
        // no blocks / no Info / not EBML / garbage / truncated header
        let mut f2 = ebml();
        f2.extend(unk(&[0x18, 0x53, 0x80, 0x67], &info(&[])));
        assert!(with_duration(&f2).is_none());
        let mut f3 = ebml();
        f3.extend(unk(&[0x18, 0x53, 0x80, 0x67], &cluster(0, &[0, 20], true)));
        assert!(with_duration(&f3).is_none());
        assert!(with_duration(b"RIFF....WAVE").is_none());
        assert!(with_duration(b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81\x01webm-test").is_none());
        assert!(with_duration(&[0x1A, 0x45, 0xDF]).is_none());
        assert!(with_duration(&[]).is_none());
        // a cut last block (a recording stopped mid-write) is ignored, the rest counts
        let mut seg4 = info(&[]);
        seg4.extend(cluster(0, &[0, 20, 40], true));
        let mut f4 = ebml();
        f4.extend(unk(&[0x18, 0x53, 0x80, 0x67], &seg4));
        f4.truncate(f4.len() - 3);
        assert_eq!(duration_of(&with_duration(&f4).unwrap()), Some(40.0));
    }

    #[test]
    fn webmdur_in_place_matches_and_leaves_refusals_untouched() {
        // add_duration edits the buffer it is given (codeant Major on #226); a refusal leaves it byte-identical
        let mut seg = info(&[]);
        seg.extend(cluster(0, &[0, 20, 40], true));
        let mut f = ebml();
        f.extend(el(&[0x18, 0x53, 0x80, 0x67], &seg)); // known-size Segment: its size field grows too
        let mut v = f.clone();
        assert!(add_duration(&mut v));
        assert_eq!(duration_of(&v), Some(60.0));
        let (_, sd, ssz) = header(&v, ebml().len()).unwrap();
        assert_eq!(sd + ssz.unwrap() as usize, v.len(), "the Segment size counts the grown Info");
        let before = v.clone();
        assert!(!add_duration(&mut v), "a real Duration is kept");
        assert_eq!(v, before);
        let mut w = b"RIFF....WAVE".to_vec();
        assert!(!add_duration(&mut w));
        assert_eq!(w, b"RIFF....WAVE");
    }
}
