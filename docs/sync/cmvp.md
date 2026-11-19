# Opensidian Sync — CMVP certificate check for aws-lc-rs `fips` (M0)

Status: notes from public sources, retrieved 2026-11-18. **Nothing in this
file is a compliance claim.** §1–§6 are the M0 evaluation; §7 was the M2
pin decision (aws-lc-rs 1.18.1 → AWS-LC 4.2.0, CMVP in process), SUPERSEDED
by §9 (2026-11-19): aws-lc-rs 1.16.0 / aws-lc-fips-sys 0.13.11 → AWS-LC FIPS
3.1.0 = the module of certificate **#5314** (validated).

Legend: **[V]** verified from a public source listed below,
**[U]** unknown / not verifiable from public sources, **[I]** our inference.

## 1. Sources (public only)

- S1 NIST CMVP certificate page #5314 (AWS-LC 3 Cryptographic Module, static)
  `https://csrc.nist.gov/projects/cryptographic-module-validation-program/certificate/5314`
- S2 NIST CMVP certificate page #4816 (AWS-LC Cryptographic Module, static)
  `https://csrc.nist.gov/projects/cryptographic-module-validation-program/certificate/4816`
- S3 NIST CMVP Modules In Process list (page "Last Updated: 11/18/2026")
  `https://csrc.nist.gov/Projects/cryptographic-module-validation-program/modules-in-process/Modules-In-Process-List`
- S4 aws-lc `crypto/fipsmodule/FIPS.md` (branch main) — vendor's own cert list
- S5 aws-lc-rs `aws-lc-fips-sys/README.md` at tag v1.18.1
- S6 aws-lc-rs git tags: submodule `aws-lc-fips-sys/aws-lc` commit per tag,
  and `AWSLC_VERSION_NUMBER_STRING` in `include/openssl/base.h` at that commit
- S7 crates.io versions API (publish dates) for aws-lc-rs, aws-lc-fips-sys, rustls
- S8 Security Policy PDF 140sp5314.pdf (downloaded, sha256
  `9e0c9f57f5d319c0c86492e6196aa5625a1b97867843e0e0d9bb423937e6f34e`;
  NOT read — no PDF tooling in the build VM, see §6)

## 2. Certificates (AWS-LC, FIPS 140-3, level 1, software)

Per S4 the vendor lists: v1.0 #4631, NetOS dynamic #5146, v2.0 dynamic
#5429, v2.0 static #4816, v3.1 dynamic #5298, v3.1 static #5314. [V, vendor claim]

Static builds are what aws-lc-fips-sys links on Linux (S5), so the static
certs matter:

- **#5314 "AWS-LC 3 Cryptographic Module (static)"** [V S1]
  - status Active; initial validation 6/5/2026 (lab: atsec); sunset 6/4/2031
  - approved list includes AES-GCM, AES-KW/KWP, Counter DRBG, HMAC-SHA2-256,
    SHA2-256, KDA HKDF SP800-56Cr1, KDF SP800-108, ECDSA KeyGen/SigGen
    (FIPS186-5)/SigVer, **EDDSA KeyGen / SigGen / SigVer**, ML-KEM, PBKDF
  - caveat (verbatim, abridged): approved mode only, configured per SP §11.1;
    "No assurance of the minimum strength of generated SSPs"; "No assurance of
    minimum security of SSPs … that are externally loaded, or of SSPs
    established with externally loaded SSPs."
- **#4816 "AWS-LC Cryptographic Module (static)"** (v2.0) [V S2]
  - status Active; initial validation 10/1/2024; sunset 9/30/2029
  - approved list has ECDSA but **no EDDSA**

In process (S3) [V]: "AWS-LC 4 Cryptographic Module" static + dynamic
(Comment Resolution - CMVP, 11/3 and 11/5/2026); "AWS-LC 5 Cryptographic
Module" static + dynamic (Review, 10/28 and 10/29/2026). In process is not
validated.

## 3. Which AWS-LC does each aws-lc-rs release actually vendor?

From S6 (submodule commit → version string) and S7 (publish dates):

- aws-lc-rs 1.13.0 / fips-sys 0.13.5 → AWS-LC 3.0.0 (rs published 2025-04-01)
- aws-lc-rs 1.14.0 / fips-sys 0.13.7 → AWS-LC 3.0.0 (2025-09-09)
- aws-lc-rs 1.15.0 / fips-sys 0.13.9 → AWS-LC 3.0.0 (2025-11-12)
- aws-lc-rs 1.16.0 / fips-sys 0.13.11 → AWS-LC 3.1.0, commit fddabb20…
  (2026-02-18) — note: NOT the commit of tag AWS-LC-FIPS-3.1.0 (ae0cd62f…)
- aws-lc-rs 1.17.4 / fips-sys 0.13.17 → AWS-LC 3.6.0, commit 76b306e7… =
  tag AWS-LC-FIPS-3.6.0 (2026-09-01)
- aws-lc-rs 1.18.0 / fips-sys 0.14.0 → AWS-LC **4.1.0** (2026-08-07)
- aws-lc-rs 1.18.1 / fips-sys 0.14.2 → AWS-LC **4.2.0**, commit 191eafc5…
  "[FIPS 4.x] Prepare v4.2.0" (2026-09-01)

**Finding 1 [V]:** the fips-sys README at v1.18.1 still says it binds
"AWS-LC-FIPS 3.0.x", but the vendored source is 4.2.0. The README is stale;
trust the submodule, not the prose.

**Finding 2 [V+I]:** the 1.18.x line ships the 4.x module, which per S3 is
in process, not validated. A build on aws-lc-rs ≥ 1.18.0 is therefore **not**
running a validated module today.

**Finding 3 [V]:** newest aws-lc-rs that vendors a 3.x module and passes the
6-week rule (cutoff 2026-10-07): **1.17.4** (fips-sys 0.13.17, AWS-LC 3.6.0).
1.17.2 is yanked. rustls with `fips` (feature maps to `aws-lc-rs?/fips`):
newest stable ≥ 6 weeks is 0.23.44 (2026-09-07) [V S7]; 0.23.45 (2026-09-14)
is also past the cutoff; 0.24.0-dev.* are pre-releases, excluded.

## 4. Unknowns (must be closed before M2 picks a pin)

- [U] Is AWS-LC 3.6.0 inside cert #5314's validated version set? FIPS.md
  calls #5314 "v3.1"; the exact software version string(s) and the
  tested operating environments are in the Security Policy (S8), not on
  the certificate page. Not read yet.
- [U] Does #5314 cover 3.1.0 at commit fddabb20 (what aws-lc-rs 1.16.0
  vendors) or only the tag ae0cd62f? Depends on SP's version string.
- [U] Tested operating environments (OS × CPU) for #5314 — we need
  Linux x86_64 + aarch64 (glibc), macOS (x86_64/arm64), Windows x86_64.
  Unknown which are listed; vendor-affirmed porting may be the only path
  for some.
- [U] EdDSA scope details: #5314 lists EDDSA KeyGen/SigGen/SigVer [V], but
  curve (Ed25519 vs Ed448) and any pre-hash restriction are in the CAVP
  algorithm cert, not read. [I] almost certainly Ed25519.
- [U] Whether the approved-mode configuration (SP §11.1) allows importing
  a raw Ed25519 seed derived by our HKDF from the recovery phrase (K_destroy)
  — the caveat says no assurance for externally loaded SSPs; see
  open-questions.md.
- [U] When the AWS-LC 4 certificate issues and its sunset date.
- [U] Whether aws-lc-rs's FIPS self-test failure surfaces as a recoverable
  error or an abort (affects syncd UX).
- [U] Android: no AWS-LC cert covering Android found (deferred, plan §4).

## 5. Consequences for the spec

- Suite `0x01` keeps Ed25519 (spec.md §1): #5314 lists EdDSA [V]; if M2
  pins to a module without EdDSA (e.g. v2.0 #4816) the suite falls back to
  ECDSA P-256 (key_alg byte already in the cfg record).
- Pin candidates for M2 (decision NOT made here): either aws-lc-rs =1.17.4
  (3.6.0, cert scope unknown) or a 1.16.x (3.1.0, closest to the "v3.1"
  label) — choose after reading the SP. Avoid 1.18.x until AWS-LC 4 is
  validated.
- FIPS builds: glibc only (plan §9), CMake + Go + cc in the build image.

## 6. How to close the unknowns

1. Read 140sp5314.pdf (sha256 above): version string(s), §11.1 config,
   operating environments, SSP import rules. Needs a PDF reader on a
   machine with one; record findings here, quoting section numbers.
2. Look up the CAVP algorithm certs referenced on S1 (A6283…A6306) for the
   EDDSA curve parameters.
3. Re-run §3 on the chosen tag (submodule commit + base.h version string).

## 7. M2 pin decision (2026-11-18)

Rule from the sync goal: pin the **newest** aws-lc-rs / aws-lc-fips-sys
released ≥ 6 weeks before 2026-11-18 (cutoff 2026-10-07), and record what
CMVP status its module has.

- **Pinned: aws-lc-rs `=1.18.1` + aws-lc-fips-sys `=0.14.2`** (both
  published 2026-09-01) [V S7]. Younger releases (1.18.2 2026-10-12, 1.19.0
  2026-10-28) fail the age rule.
- **Module: AWS-LC 4.2.0** (submodule commit 191eafc5…, "[FIPS 4.x]
  Prepare v4.2.0") [V S6].
- **CMVP status: IN PROCESS, not validated.** "AWS-LC 4 Cryptographic
  Module" (static + dynamic) is on the Modules In Process list, status
  Comment Resolution - CMVP (11/3 and 11/5/2026) [V S3]. No certificate
  number exists for it yet. #5314 (AWS-LC 3, v3.1) and #4816 (v2.0) do
  **not** cover 4.2.0 [I: different major version / module name].
- Consequence: builds of this branch run the AWS-LC FIPS module in approved
  mode, but **no FIPS 140-3 validation claim may be made** until the AWS-LC 4
  certificate issues and its Security Policy lists 4.2.0 (or the pin moves to
  a version it lists). Re-check S3 / S4 before any release.
- Rejected alternative: aws-lc-rs 1.17.4 (AWS-LC 3.6.0, same publish day).
  Whether #5314 covers 3.6.0 is still [U] (§4), so it is not provably
  validated either, and it is not the newest — the goal rule picks 1.18.1.

### Signature algorithm (spec.md §1, key_alg byte)

The goal rule is "Ed25519 if the pinned cert covers it, else P-256". The
pinned module has **no certificate**, so no cert covers Ed25519 for it →
suite v1 signs with **ECDSA P-256 / SHA-256 (FIPS 186-5)**, key_alg = P-256.
Ed25519 stays reserved in the key_alg byte; switching back is a cfg/{gen}
change once an AWS-LC 4 certificate lists EdDSA.

### Build toolchain (box only, deps.md §Toolchain)

aws-lc-fips-sys in FIPS mode requires CMake, Go **and Perl**
(`aws-lc/CMakeLists.txt`: "Building AWS-LC for FIPS requires Go and Perl")
plus a C compiler; C++ is not needed (tests/libssl off). Pinned versions and
sha256 are in deps.md. Nothing is installed on the development VM.

## 8. M2d: raw key import + phrase-derived keys (open questions 2, 10, 12)

Checked on the box against the pinned code (aws-lc-rs 1.18.1 source in the
cargo registry, AWS-LC 4.2.0 FIPS build) and by tests that run in FIPS mode
(`crates/sync-crypto/tests/p256_derive.rs`, `try_fips_mode()` asserted).

- **Q10 — raw private-key import in FIPS mode: permitted by the module.**
  aws-lc-rs exposes `EcdsaKeyPair::from_private_key_der` (RFC 5208 / RFC
  5915) and `from_private_key_and_public_key` (raw big-endian scalar +
  point). With the `fips` feature, `from_private_key_der` calls
  `validate_ec_evp_key` → `EC_KEY_check_fips` (pairwise-consistency test)
  before the key is usable [V: source, ec/key_pair.rs]. We use the RFC 5915
  path without a public key (AWS-LC computes Q = d·G), so no
  caller-supplied point is trusted. Box test: d = 1 yields Q = G, d = n − 1
  yields −G, d ∈ {0, n, n+1, 2²⁵⁶−1} is rejected (range guard before the
  import), an imported key signs and
  verifies and survives a PKCS#8 round trip.
- **Decision: K_destroy and K_rsig stay derived from the phrase** (no switch
  to module-generated keys). Reason: destroy (spec §5) and cfg recovery must
  be provable from the backup words alone on a fresh device; a
  module-generated key whose private half lives only on one device cannot
  do that. Device signing keys and VK are still module-generated (DRBG).
- **Q12 — P-256 scalar derivation:** counter-retry kept, but re-shaped to
  the FIPS 186-5 A.2.2 arithmetic (accept c ≤ n − 2, d = c + 1; uniform on
  [1, n − 1]); candidate bits = HKDF-SHA256 in the module (SP 800-56C). The
  difference to A.2.2 is the bit source (KDF of a high-entropy secret, not
  RBG output). For the security policy this is an **SSP entered into the
  module**, the case the #5314 caveat gives "no assurance" for. Recorded,
  not hidden: when a certificate exists (§7), the claim is "approved
  algorithms, keys derived outside the A.2 generation service".
- **Q2 — certificate scope:** closed for v1 by §7: the pinned module (AWS-LC
  4.2.0) has **no certificate**, so the #5314 SP-version / tested-environment
  questions do not apply to this pin. Follow-up before any release: when the
  AWS-LC 4 certificate issues, check that its SP lists 4.2.0 (or move the
  pin), its tested environments (Linux x86_64 / aarch64 glibc), whether it
  lists EdDSA (→ suite 0x01), and whether its caveats change the Q12 note.

## 9. Re-pin to the validated module (2026-11-19, operator decision B)

Operator decision 2026-11-19 07:41Z: use the already validated
aws-lc-fips-sys 0.13.x line (newest release ≥ 6 weeks old that maps to a
CMVP-validated certificate, not "in process"). This closes the §4 unknowns
for that line. All sources public; retrieved 2026-11-19.

Sources added:
- S9 Security Policy `140sp5314.pdf` (NIST CSRC), sha256
  `9e0c9f57f5d319c0c86492e6196aa5625a1b97867843e0e0d9bb423937e6f34e`
  (same file as S8), now READ (text extracted from its Flate streams).
- S10 `https://github.com/aws/aws-lc/archive/refs/tags/AWS-LC-FIPS-3.1.0.zip`
- S11 crates.io crate files `aws-lc-fips-sys-0.13.{11,17}.crate` and the
  aws-lc-rs repo tags v1.15.3..v1.16.1 (`aws-lc-fips-sys/Cargo.toml`).

**Certificate #5314** [V S1, S9]: "AWS-LC 3 Cryptographic Module (static)",
Amazon Web Services Inc., FIPS 140-3, overall level 1, status Active,
initial validation 6/5/2026 (atsec), sunset 6/4/2031.
- Module version [V S9 §2.1, Table 2]: the cryptographic boundary is
  `bcm.o`, **version "AWS-LC FIPS 3.1.0"** — the only version listed.
- Source [V S9 §11.1]: built from `AWS-LC-FIPS-3.1.0.zip` (S10), expected
  sha256 `fe408fa438850786396faf79eba9ea4116c3802e60f3a95865f0dd2adb64c9f1`,
  "There shall be no additions, deletions, or alterations of this set";
  build `cmake -DFIPS=1`.
- Tested operational environments [V S9 Table 3]: Amazon Linux 2023 on
  c6i.metal (Intel Xeon Platinum 8375C, with and without PAA) and on
  r8g.metal-24xl (Graviton4); **no vendor-affirmed OEs**. Our box (Fedora 44,
  AMD EPYC cpx51) and user desktops are NOT tested OEs: "CMVP makes no
  statement as to the correct operation of the module … when so ported".
- Approved algorithms used by suite v1 [V S9 Table 5]: AES-GCM, HKDF
  (KDA SP800-56Cr1), HMAC-SHA2-256, SHA2-256, Counter DRBG, ECDSA
  KeyGen/SigGen/SigVer (FIPS 186-5, P-256), **EDDSA KeyGen/SigGen/SigVer
  curve ED-25519** (FIPS 186-5).

**Which crate vendors exactly that source** [V S11, box check
`logs/fipsverify-0.13.11-vs-5314.log` in the goal dir]:
- S10 downloaded on the box: sha256 = `fe408fa4…c9f1` = the SP value.
- `AWSLC_VERSION_NUMBER_STRING` per fips-sys release: 0.13.10 3.0.0,
  **0.13.11 3.1.0**, 0.13.12 3.2.0, 0.13.13/14 3.3.0, 0.13.15/16 3.4.0,
  0.13.17 3.6.0.
- 0.13.11 vs S10, whole tree: **every file the crate ships is byte-identical**
  to the zip except `go.mod` (outside the module; Go is only the build-time
  delocator/inject_hash tooling). The crate omits 257 files (test vectors,
  docs, images) — none of them is a `bcm.o` input.
- 0.13.17 vs S10: 29 files differ, including module sources inside
  `crypto/fipsmodule` (`cipher/cipher.c`, `cipher/e_aesccm.c`, `dh/dh.c`,
  `evp/p_hkdf.c`, `evp/p_kem.c`) → 3.6.0 is NOT the validated module.
- aws-lc-rs tags whose tree ships fips-sys 0.13.11: v1.15.3, v1.15.4,
  **v1.16.0** (v1.16.1 ships 0.13.12). Newest = **1.16.0** (2026-02-18).

**Pinned:** `aws-lc-rs =1.16.0` + `aws-lc-fips-sys =0.13.11` (2026-01-14),
both well past the 6-week rule. Module = AWS-LC FIPS 3.1.0 = cert #5314.

What may be claimed (and what not):
- MAY: "uses the AWS-LC FIPS 3.1.0 module (CMVP certificate #5314), built
  from source identical to the validated source set, in approved mode".
- NOT: "FIPS 140-3 validated on <user platform>" — only the two Amazon
  Linux 2023 OEs are tested; any other OS/CPU is a porting case without a
  CMVP statement. The aws-lc-fips-sys build adds its own CMake flags around
  `-DFIPS=1` (not the SP's exact command line).
- Q12 caveat unchanged (§8): phrase-derived K_destroy / K_rsig are SSPs
  entered into the module; #5314 caveat "no assurance of the minimum
  strength of generated SSPs" / externally loaded SSPs applies.

**Signature algorithm (goal rule "Ed25519 if the pinned cert covers it, else
P-256"):** #5314 lists EDDSA ED-25519 [V S9], so the rule now selects
**Ed25519** for suite v1; the switch from the §7 P-256 fallback is a
separate change (key_alg byte, keytree derivation, KAT vectors), tracked in
the goal ledger.

**Known advisories on the pinned line** (RustSec advisory-db, checked
2026-11-19) — see deps.md §FIPS-0.13 for reachability:
RUSTSEC-2026-0042 (CRL distribution-point check, patched 0.13.13) and
RUSTSEC-2026-0043 (AES-CCM tag timing via EVP_CIPHER, patched 0.13.12).
Both are outside the AWS-LC module code paths suite v1 calls (no AES-CCM,
no AWS-LC X.509/CRL — certificates are verified by rustls-webpki).
