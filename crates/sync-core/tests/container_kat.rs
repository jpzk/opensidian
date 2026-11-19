// SPDX-License-Identifier: GPL-3.0-or-later
//! Open-direction known answers for object containers:
//! crates/sync-core/vectors/container.vec (osy-vec-1, docs/sync/test-vectors.md).
//! Each container must open to exactly `body` on the FIPS module; the header
//! commitment is recomputed with the independent SoftProvider (sha2); and
//! every single-byte flip of each container must be rejected.
use std::collections::BTreeMap;

use sync_core::object::{open, ObjectCtx, ObjectError};
use sync_crypto::fips::FipsProvider;
use sync_crypto::soft::SoftProvider;
use sync_crypto::{derive_object_keys, key_commitment};
use sync_proto::container::{Comp, Kind, Suite, FMT_V1, HEADER_LEN};
use sync_proto::Id16;

type Case = BTreeMap<String, String>;

fn load(name: &str) -> Vec<(String, Case)> {
    let path = format!("{}/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let (mut format_ok, mut area_ok) = (false, false);
    let mut cases: Vec<(String, Case)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(id) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cases.push((id.to_string(), Case::new()));
            continue;
        }
        let (k, v) = line.split_once(" = ").unwrap_or_else(|| panic!("bad line {line:?}"));
        match cases.last_mut() {
            Some((_, c)) => assert!(c.insert(k.into(), v.into()).is_none(), "dup {k}"),
            None => match k {
                "format" => format_ok = v == "osy-vec-1",
                "area" => area_ok = v == "container",
                "suite" => assert_eq!(v, "02"),
                _ => panic!("unknown header key {k}"),
            },
        }
    }
    assert!(format_ok && area_ok, "{name}: header");
    cases
}

fn unhex(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0, "odd hex");
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn arr<const N: usize>(c: &Case, k: &str) -> [u8; N] {
    unhex(&c[k]).try_into().unwrap_or_else(|_| panic!("{k} length"))
}

#[test]
fn container_kat() {
    let p = FipsProvider::new().expect("FIPS provider");
    let cases = load("container.vec");
    assert_eq!(cases.len(), 2);
    let mut zstd_seen = false;
    for (id, c) in &cases {
        assert_eq!(c["result"], "valid", "{id}");
        let k: [u8; 32] = arr(c, "k_root");
        let ctx = ObjectCtx {
            suite: Suite::P256,
            vault_id: Id16(arr(c, "vault_id")),
            obj_id: Id16(arr(c, "obj_id")),
        };
        let obj = unhex(&c["container"]);
        let body = unhex(&c["body"]);
        let o = open(&p, &k, &ctx, &obj).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(o.body, body, "{id}");
        assert_eq!(o.kind as u8, Kind::Batch as u8, "{id}");
        assert_eq!(format!("{:02x}", o.comp as u8), c["comp"], "{id}");
        zstd_seen |= o.comp == Comp::Zstd;

        // independent commitment: sha2 HKDF/HMAC reference, not OpenSSL
        let s = SoftProvider;
        let keys = derive_object_keys(&s, &k, &ctx.obj_id.0, 0x02).unwrap();
        let want = key_commitment(&s, &keys.commit, &ctx.vault_id.0, &ctx.obj_id.0, 0x02, FMT_V1);
        assert_eq!(&obj[24..56], &want[..], "{id}: commitment");
        assert_eq!(&obj[..8], &[b'O', b'S', b'Y', b'N', 0x01, 0x02, 0, 0], "{id}");

        for i in 0..obj.len() {
            let mut t = obj.clone();
            t[i] ^= 0x01;
            let r = open(&p, &k, &ctx, &t);
            assert!(r.is_err(), "{id}: flip at {i} accepted");
            if i >= HEADER_LEN {
                assert_eq!(r, Err(ObjectError::Auth), "{id}: flip at {i}");
            }
        }
        assert!(open(&p, &k, &ctx, &obj[..obj.len() - 1]).is_err());
    }
    assert!(zstd_seen, "a zstd vector must be present");
}
