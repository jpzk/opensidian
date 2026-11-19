# Opensidian Sync — CMVP certificate for the OpenSSL 3 FIPS provider

Status: notes from public sources, retrieved 2026-11-19. **Nothing in this
file is a compliance claim.** This file replaces the AWS-LC evaluation
(operator decision 2026-11-19 10:16Z, "okay openssl it is": aws-lc-fips-sys
carries the OpenSSL/SSLeay licence, which is GPL-incompatible; OpenSSL 3 is
Apache-2.0). The AWS-LC text is in git history (cmvp.md before this change).

Legend: **[V]** verified from a public source listed below,
**[U]** unknown / not verifiable from public sources, **[I]** our inference.

## 1. Sources (public only)

- S1 CMVP certificate pages, csrc.nist.gov
  `/projects/cryptographic-module-validation-program/certificate/<n>` for
  #4282, #4985, #5586, and the advanced search Vendor=OpenSSL, status Active.
- S2 Security Policy `140sp5586.pdf` (CSRC), document version 1.2, sha256
  `02c3dba6cce548ab270368e07038b2ba50d1868f733e2037a95be15892448fd4`
  (text read from its Flate streams).
- S3 `https://openssl-library.org/source/` — table of FIPS-validated
  versions, release signing key, rule "other OpenSSL releases MAY use the
  validated FIPS provider, but MUST NOT build and use their own".
- S4 `https://github.com/openssl/openssl/releases/tag/openssl-3.5.9`
  (published 2026-09-29) and `https://www.openssl.org/source/openssl-3.5.9.tar.gz`.

## 2. Certificates (OpenSSL FIPS Provider)

Active OpenSSL certificates on 2026-11-19 [V S1]: **#4985** and **#5586**.
(#4282, provider 3.0.8/3.0.9, FIPS 140-2: Historical, sunset.)

- **#4985** — provider 3.1.2 [V S3], FIPS 140-3 level 1, Active, initial
  2025-03-11, sunset 2030-03-10. Approved list has ECDSA (FIPS 186-4) but
  **no EdDSA**.
- **#5586** — "OpenSSL FIPS Provider", The OpenSSL Corporation, FIPS 140-3,
  overall level 1, software, multi-chip standalone, **Active**, initial
  validation **2026-11-13** (Lightship Security), **sunset 2031-11-12**
  [V S1]. Module version **3.5.9**: `fips.so` 3.5.9 (Linux/FreeBSD),
  `fips.dll`, `fips.dylib`; integrity test HMAC-SHA2-256 [V S2 §2.2 Table 2].
  Caveat: "When operated in approved mode. No assurance of the minimum
  strength of generated SSPs (e.g., keys). No assurance of minimum security
  of SSPs (e.g., keys, bit strings) that are externally loaded, or of SSPs
  established with externally loaded SSPs" [V S1].

**Pinned: provider 3.5.9, certificate #5586.** Reason: newest active
certificate, the only one that lists EdDSA (rule d below), longest sunset.
The OpenSSL core is built from the same 3.5.9 release (no newer core is
needed; S3 would allow one).

### Approved algorithms suite v1 uses [V S1, S2 §2.5, CAVP A9077]

- AES-GCM (AES-256) — object/container encryption, TLS 1.3 record layer
- KDA HKDF SP 800-56Cr2 (HKDF-SHA256) — key tree, TLS 1.3 key schedule
- HMAC-SHA2-256 (key length 112–524288 bits)
- SHA2-256
- Hash DRBG / HMAC DRBG / Counter DRBG — all random bytes (`RAND_bytes`)
- **EDDSA KeyGen / KeyVer / SigGen / SigVer, curve ED-25519**, pure and
  pre-hash (FIPS 186-5) — suite 0x01 signatures
- ECDSA KeyGen / KeyVer / SigGen / SigVer (FIPS 186-5), P-256 — suite 0x02,
  TLS server certificates
- TLS v1.3 KDF

Not approved [V S2 §2.5 "non-approved, not allowed"]: HKDF or HMAC with a
key shorter than 112 bits. Consequence: KAT vectors with short keys (RFC 4231
test case 2, key "Jefe") are not approved-mode vectors and are kept out of
the approved-mode KAT set (test-vectors.md).

### Tested operational environments [V S2 Table 3]

Each with and without PAA, no hypervisor: Ubuntu 22.04.1 Server (Intel
i7-8550U), Ubuntu 24.04.2 LTS (AMD EPYC 4124P), Debian 11.5 (i7-8550U),
Debian 12 (EPYC 4124P), FreeBSD 13.1 (i7-10510U), FreeBSD 14.3 (EPYC 4124P),
Windows 10 Pro (i7-10510U), Windows Server 2025 (EPYC 4124P), macOS 15.5
(Apple M4). **No vendor-affirmed OEs.** Our build box (Fedora, AMD EPYC
VM) and most user desktops are not tested OEs — a porting case without a
CMVP statement [V S2 Appendix A].

## 3. Source, signature, build (box only)

- Release `openssl-3.5.9`, published 2026-09-29 [V S4] — 51 days before
  2026-11-19 (6-week rule met).
- `openssl-3.5.9.tar.gz` sha256
  **`603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a`**
  (GitHub release asset and openssl.org/source hashed independently; equals
  the published `.sha256`).
- Signature `openssl-3.5.9.tar.gz.asc`: signing subkey
  `C46E D3F2 CBEF DA1F DAAD A442 64ED 7B1D CCE7 1CB2`, certified by the
  OpenSSL signing certificate, primary
  **`B146 647E 45A7 B339 47AB 226B 2A2C 87D1 6169 2D40`** (S3: "the
  canonical trust anchor"). Verified with gpg on the box.
- Build per SP §11.1: `./Configure enable-fips` → `make` → `make install`
  into a box-local prefix (nothing installed system-wide, nothing on the
  agent VM), then `openssl fipsinstall -pedantic`. The box build log (sha256
  check, gpg verify, configure line, `openssl version -v`, `openssl list
  -providers`, fipsinstall output) is
  `logs/ossl-3.5.9-build.log` in the goal dir. **Box build 2026-11-19
  10:35Z (Fedora Linux 44, x86_64, gcc 16.0.1, Perl 5.44.0 box-built):**
  - `sha256sum -c` → OK; gpg `VALIDSIG C46ED3F2…1CB2 … B146647E45A7B33947AB226B2A2C87D161692D40`
    (key fetched by fingerprint from keys.openpgp.org into a throwaway GNUPGHOME).
  - `./Configure enable-fips --prefix=/var/tmp/sync-e2e/ossl/openssl-3.5.9
    --openssldir=…/ssl --libdir=lib` (no other options), `make`,
    `make install_sw install_fips`.
  - `openssl fipsinstall -pedantic` → `OpenSSL FIPS Provider version 3.5.9
    build 3.5.9`, `INSTALL PASSED`; `-verify` → `VERIFY PASSED`.
    fipsmodule.cnf: `security-checks = 1`, `conditional-errors = 1`,
    `drbg-no-trunc-md = 1`, `tls1-prf-ems-check = 1`, `hmac-key-check = 1`,
    `hkdf-key-check = 1` (+ the other -pedantic checks).
  - `openssl list -providers`: `fips` (OpenSSL FIPS Provider 3.5.9, active)
    + `base` (3.5.9, active); `default_properties = fips=yes`: SHA-256 and
    Ed25519 keygen work, MD5 is refused.
  - sha256: `fips.so` `23666daba01ab5ece657941799e4607a125db3656045124b290e117a4fa495c6`,
    `libcrypto.so.3` `dcb13813579a298f01edbafa0e0eec309bc56f1c34dad02c8d2081660a1c8607`,
    `libssl.so.3` `8f5a783c4d4028536683a3a55139f78563f7e510bd52cd8ccf214876dda0507c`
    (per-build values; the module-mac in fipsmodule.cnf is regenerated by
    fipsinstall on every install, never shipped from another machine).
  - Fedora 44 is not a tested OE of #5586 (see Tested OEs above).
- `OPENSSL_NO_FIPS_SECURITYCHECKS` must never be defined [V S2 §11.1c].

## 4. Operational rules (what the code and packages must do)

- **Configuration** [V S2 §2.4, §11.1]: `fipsmodule.cnf` is produced by
  `openssl fipsinstall` for the installed `fips.so`; it holds the module's
  HMAC-SHA2-256 integrity value and the approved-mode settings
  `security-checks = 1`, `conditional-errors = 1`, `drbg-no-trunc-md = 1`,
  `tls1-prf-ems-check = 1`. It must not be edited by hand; changing those
  settings makes the module non-compliant.
- **Self-tests**: run on module load (integrity check over `fips.so` +
  KATs); conditional PCTs on key generation and on EdDSA/ECDSA key import;
  on-demand by reloading the module. A failed self-test puts the module in
  the error state — no crypto output.
- **Approved mode in sync** (code, `sync-crypto`): before any sync work the
  process loads the `fips` provider (plus `base` for encoders), sets the
  default properties to `fips=yes`, and checks that the fips provider is
  available and `EVP_default_properties_is_fips_enabled` is 1. If not,
  sync **refuses to start** (`syncd`, `sync-server`, the app's sync window).
  Every primitive is fetched through EVP under `fips=yes`; there is no
  fallback to the default provider.
- **Status indicator**: the approved-service indicator is the fips provider
  answering a fetch under `fips=yes`; non-approved requests fail instead of
  silently using another provider.

## 5. Raw key import and phrase-derived keys (open questions 10, 12)

- **Q10 — raw private-key import**: the provider accepts Ed25519 private
  keys as raw 32-byte seeds (`EVP_PKEY_new_raw_private_key`) and P-256
  private keys as RFC 5915 / PKCS#8 encodings; it runs a pairwise
  consistency test on EdDSA/ECDSA key import [V S2, PCT on key import]. Raw import is a
  key-*entry* by the calling application over the API (the SP's "manual key
  entry is not supported" means operator keyboard entry). Box test PENDING
  (ledger item 3: the RFC 8032 seed-import KATs through the provider).
- **Decision unchanged**: K_destroy and K_rsig stay phrase-derived (destroy
  and recovery must work from the words alone); device keys and VK are
  module-generated (DRBG).
- **Q12**: P-256 scalar derivation (suite 0x02 only) keeps the FIPS 186-5
  A.2.2 arithmetic with HKDF output as the bit source. Phrase-derived keys
  are **externally loaded SSPs** — exactly the case the #5586 caveat gives
  "no assurance" for. Recorded, not hidden.
- HKDF input keys and HMAC keys must be ≥ 112 bits in approved mode; the
  phrase-derived IKM and every HMAC key in suite v1 are ≥ 256 bits.

## 6. Signature algorithm (rule d)

#5586 lists EDDSA ED-25519 as approved [V S1, S2] → **Ed25519 suite 0x01
stays the v1 suite**. P-256 (suite 0x02) remains for verification and as the
fallback should a future pin lose EdDSA (#4985 would force that switch).

## 7. Packaging — what each artifact must carry (rule f)

The provider is a loadable module: `fips.so` + `fipsmodule.cnf` +
an `openssl.cnf` that includes it and activates `fips` and `base`. The
release artifact set does not change (operator approval needed for that).

- **sync-server** (tarball / container): `fips.so`, run `openssl
  fipsinstall -pedantic` at install time on the target host, ship an
  `openssl.cnf` stub; set `OPENSSL_CONF` / `OPENSSL_MODULES` in the unit.
- **deb / rpm**: ship `fips.so` under the package's lib dir and run
  fipsinstall in the post-install script (the integrity value is per
  installed file); the distro's own OpenSSL is not used.
- **AppImage**: bundle `fips.so`; at first start write `fipsmodule.cnf`
  into the user config dir via the bundled fipsinstall logic, then point
  `OPENSSL_CONF` / `OPENSSL_MODULES` there before sync starts.
- **flatpak**: build OpenSSL 3.5.9 with `enable-fips` as a module of the
  manifest (pinned sha256), install `fips.so` in `/app/lib/ossl-modules`,
  generate `fipsmodule.cnf` at first run into `$XDG_CONFIG_HOME`.
- In every case: if the provider cannot be loaded the app still runs, but
  sync refuses to start with a clear message (§4).
