#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# android-sbom.sh self-test, offline (bash + jq; no cargo, no gradle, no network).
# The fixture checkout is written into a temp dir by this script (no build.gradle / Cargo.lock is
# committed under tests/, so GitHub's dependency graph never picks fixture coordinates up):
#   Cargo.lock: opensidian -> jni, libc, serde, gtk -> glib (path-patched); aarch64 resolve: no gtk, no glib
#   Gradle: classpath AGP + kotlin plugin, buildSrc AGP, implementation webkit/appcompat,
#           testImplementation junit, apply(from = "tauri.build.gradle.kts") (absent, gitignored),
#           wrapper 8.14.3 with distributionSha256Sum, RustPlugin targetList aarch64,armv7,x86_64
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
S="$here/android-sbom.sh"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
export SOURCE_DATE_EPOCH=1700000000
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
check() { if jq -e "$3" "$2" >/dev/null; then ok "$1"; else bad "$1"; fi; }
expect_fail() { local name=$1 pat=$2; shift 2
  set +e; "$@" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$rc" -ne 0 ] && [ ! -s "$t/so" ] && grep -q -- "$pat" "$t/se"; then ok "$name: rc=$rc"
  else cat "$t/se"; bad "$name: rc=$rc stdout=$(wc -c <"$t/so")B (want !=0, empty stdout, '$pat')"; fi; }

mk_checkout() { # mk_checkout <dir>
  local c=$1 g=$1/src-tauri/gen/android
  mkdir -p "$g/app" "$g/buildSrc/src/main/java/x" "$g/gradle/wrapper" "$c/ui/vendor"
  cat >"$c/src-tauri/Cargo.toml" <<'T'
[package]
name = "opensidian"
version = "0.9.4"
license = "GPL-3.0-or-later"
T
  cat >"$c/src-tauri/Cargo.lock" <<'T'
# fixture (not a real lockfile)
version = 4

[[package]]
name = "glib"
version = "0.18.5"

[[package]]
name = "gtk"
version = "0.18.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111111111111111111111111111111111111111111111111111111111111111"
dependencies = [
 "glib",
]

[[package]]
name = "jni"
version = "0.21.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2222222222222222222222222222222222222222222222222222222222222222"
dependencies = [
 "libc",
]

[[package]]
name = "libc"
version = "0.2.170"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3333333333333333333333333333333333333333333333333333333333333333"

[[package]]
name = "opensidian"
version = "0.9.4"
dependencies = [
 "gtk",
 "jni",
 "serde",
]

[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4444444444444444444444444444444444444444444444444444444444444444"
T
  cat >"$g/build.gradle.kts" <<'T'
buildscript {
    dependencies {
        classpath("com.android.tools.build:gradle:8.11.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.25")
        // classpath("com.example:commented-out:9.9.9")
    }
}
T
  cat >"$g/app/build.gradle.kts" <<'T'
android {
    compileSdk = 36
    defaultConfig {
        minSdk = 30
        targetSdk = 36
    }
}
dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    testImplementation("junit:junit:4.13.2")
}
apply(from = "tauri.build.gradle.kts")
T
  cat >"$g/buildSrc/build.gradle.kts" <<'T'
dependencies {
    compileOnly(gradleApi())
    implementation("com.android.tools.build:gradle:8.11.0")
}
T
  cat >"$g/buildSrc/src/main/java/x/RustPlugin.kt" <<'T'
        val targetsList = (findProperty("targetList") as? String)?.split(',') ?: listOf("aarch64", "armv7", "x86_64")
T
  cat >"$g/gradle/wrapper/gradle-wrapper.properties" <<'T'
distributionUrl=https\://services.gradle.org/distributions/gradle-8.14.3-bin.zip
distributionSha256Sum=bd71102213493060956ec229d946beee57158dbd89d0e62b91bca0fa2c5f3531
T
}
mk_meta() { # mk_meta <out> <crate>...  (resolve graph = opensidian + the given crates)
  local o=$1; shift
  jq -n --args '$ARGS.positional as $c
    | {"jni": "0.21.1", "libc": "0.2.170", "serde": "1.0.219", "gtk": "0.18.2"} as $v
    | ([{name: "opensidian", version: "0.9.4", id: "path+file:///w/src-tauri#opensidian@0.9.4", source: null}]
       + [$c[] | {name: ., version: $v[.], id: "registry+https://github.com/rust-lang/crates.io-index#\(.)@\($v[.])", source: "registry+https://github.com/rust-lang/crates.io-index"}]) as $p
    | {packages: $p, resolve: {root: $p[0].id, nodes: [$p[] | {id, deps: []}]}}' "$@" >"$o"
}
cat >"$t/vendor.cdx.json" <<'T'
{"bomFormat":"CycloneDX","specVersion":"1.6","version":1,
 "metadata":{"component":{"type":"application","bom-ref":"ui-vendor","name":"ui/vendor","version":"0.9.4"}},
 "components":[{"type":"library","bom-ref":"pkg:npm/katex@0.16.22","name":"katex","version":"0.16.22","purl":"pkg:npm/katex@0.16.22"}],
 "dependencies":[{"ref":"ui-vendor","dependsOn":["pkg:npm/katex@0.16.22"]},{"ref":"pkg:npm/katex@0.16.22","dependsOn":[]}]}
T
C=$t/co; mk_checkout "$C"
mk_meta "$t/meta.json" jni libc serde

# --- targets ---------------------------------------------------------------------------------
"$S" -L -C "$C" >"$t/targets"
if [ "$(paste -sd' ' "$t/targets")" = "aarch64-linux-android armv7-linux-androideabi x86_64-linux-android" ]; then
  ok "-L: targets from RustPlugin.kt targetList"; else bad "-L: got $(paste -sd' ' "$t/targets")"; fi

# --- happy path ------------------------------------------------------------------------------
f=$("$S" -C "$C" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/out1" 2>"$t/log1") || { cat "$t/log1"; exit 1; }
[ "$f" = "$t/out1/opensidian-0.9.4-android-aarch64.cdx.json" ] && ok "output name opensidian-<v>-android-<arch>" || bad "output name '$f'"
(cd "$t/out1" && sha256sum -c --status "$(basename "$f").sha256") && ok ".sha256 written and matches" || bad ".sha256"
check "CycloneDX 1.6 + serialNumber" "$f" '.bomFormat == "CycloneDX" and .specVersion == "1.6" and (.serialNumber | test("^urn:uuid:"))'
check "root: opensidian android, GPL-3.0-or-later" "$f" \
  '.metadata.component.name == "opensidian android" and .metadata.component.licenses == [{expression: "GPL-3.0-or-later"}]'
check "root records target + ref" "$f" \
  '[.metadata.component.properties[] | select(.name == "opensidian:rust:target" and .value == "aarch64-linux-android")] | length == 1'
check "cargo: filtered to the aarch64 resolve (jni libc serde, no gtk)" "$f" \
  '[.components[] | select(.purl // "" | startswith("pkg:cargo/")) | .name] | sort == ["jni","libc","serde"]'
check "npm: vendored JS kept" "$f" '[.components[] | select(.purl == "pkg:npm/katex@0.16.22")] | length == 1'
check "maven: 5 exact purls (comment ignored)" "$f" \
  '[.components[] | select(.purl // "" | startswith("pkg:maven/")) | .purl] | sort == ["pkg:maven/androidx.appcompat/appcompat@1.7.1","pkg:maven/androidx.webkit/webkit@1.14.0","pkg:maven/com.android.tools.build/gradle@8.11.0","pkg:maven/junit/junit@4.13.2","pkg:maven/org.jetbrains.kotlin/kotlin-gradle-plugin@1.9.25"]'
check "maven: AGP once, classpath + buildSrc merged, role build, scope excluded" "$f" \
  '[.components[] | select(.purl == "pkg:maven/com.android.tools.build/gradle@8.11.0")] | length == 1 and (.[0].scope == "excluded")
     and any(.[0].properties[]; .name == "opensidian:maven:configurations" and .value == "classpath,implementation")'
check "maven: webkit runtime -> scope required" "$f" \
  '[.components[] | select(.purl == "pkg:maven/androidx.webkit/webkit@1.14.0")][0] | .scope == "required"
     and any(.properties[]; .name == "opensidian:maven:role" and .value == "runtime")'
check "maven: no lockfile -> every coordinate flagged locked=false" "$f" \
  '[.components[] | select(.purl // "" | startswith("pkg:maven/"))] | all(any(.properties[]; .name == "opensidian:maven:locked" and .value == "false"))'
check "maven: generated tauri.build.gradle.kts named as not covered" "$f" \
  'any(.components[]; any(.properties[]?; .name == "opensidian:maven:not-covered" and (.value | test("app/tauri.build.gradle.kts"))))'
check "gradle wrapper: 8.14.3 with its sha256, no purl" "$f" \
  '[.components[] | select(.name == "gradle" and .version == "8.14.3")][0] | .purl == null and .hashes[0].content == "bd71102213493060956ec229d946beee57158dbd89d0e62b91bca0fa2c5f3531"'
check "Android System WebView: OS-provided, not bundled, Play Store, no purl" "$f" \
  '[.components[] | select(."bom-ref" == "os:android-system-webview")][0] | .purl == null and .scope == "required"
     and any(.properties[]; .name == "opensidian:bundled" and .value == "false")
     and any(.properties[]; .name == "opensidian:updated-via" and (.value | test("Play Store")))'
check "Android OS: minSdk 30 / targetSdk 36" "$f" \
  '[.components[] | select(."bom-ref" == "os:android@api-30")][0] | .version == ">= API 30"
     and any(.properties[]; .name == "opensidian:build:targetSdk" and .value == "36")'
grep -q 'UNLOCKED maven androidx.webkit:webkit:1.14.0' "$t/log1" && ok "stderr flags unlocked maven coordinates" || bad "no UNLOCKED log"

# --- deterministic ---------------------------------------------------------------------------
f2=$("$S" -C "$C" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/out2" 2>/dev/null)
[ "$(sha256sum <"$f")" = "$(sha256sum <"$f2")" ] && [ "$(cut -c1-64 "$f.sha256")" = "$(cut -c1-64 "$f2.sha256")" ] && ok "deterministic: same input -> same bytes" || bad "not deterministic"
f3=$("$S" -C "$C" -t x86_64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/out1" 2>/dev/null)
[ "$(basename "$f3")" = "opensidian-0.9.4-android-x86_64.cdx.json" ] && ok "x86_64 target -> android-x86_64" || bad "x86_64 name $f3"

# --- fail closed -----------------------------------------------------------------------------
expect_fail "unknown (non-Android) target refused" "not an Android triple" \
  "$S" -C "$C" -t aarch64-apple-darwin -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/x"
expect_fail "Android target the branch does not build refused" "not built by this checkout" \
  "$S" -C "$C" -t i686-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/x"
mk_checkout "$t/nolock"; rm "$t/nolock/src-tauri/Cargo.lock"
expect_fail "missing Cargo.lock refused" "no src-tauri/Cargo.lock" \
  "$S" -C "$t/nolock" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/x"
mk_meta "$t/meta-gtk.json" jni libc serde gtk
expect_fail "desktop crate in the resolve refused" "desktop-only crate 'gtk'" \
  "$S" -C "$C" -t aarch64-linux-android -m "$t/meta-gtk.json" -j "$t/vendor.cdx.json" -o "$t/x"
mk_meta "$t/meta-nojni.json" libc serde
expect_fail "resolve without jni refused" "no jni" \
  "$S" -C "$C" -t aarch64-linux-android -m "$t/meta-nojni.json" -j "$t/vendor.cdx.json" -o "$t/x"
jq '.packages += [{name: "ghost", version: "1.0.0", id: "registry+x#ghost@1.0.0"}] | .resolve.nodes += [{id: "registry+x#ghost@1.0.0"}]' "$t/meta.json" >"$t/meta-ghost.json"
expect_fail "resolved crate absent from Cargo.lock refused" "not in rust-sbom.sh output: ghost@1.0.0" \
  "$S" -C "$C" -t aarch64-linux-android -m "$t/meta-ghost.json" -j "$t/vendor.cdx.json" -o "$t/x"
mk_checkout "$t/mit"; sed -i 's/GPL-3.0-or-later/MIT/' "$t/mit/src-tauri/Cargo.toml"
expect_fail "root license other than GPL-3.0-or-later refused" "license 'MIT'" \
  "$S" -C "$t/mit" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/x"
expect_fail "no SOURCE_DATE_EPOCH outside git refused" "not be deterministic" \
  env -u SOURCE_DATE_EPOCH "$S" -C "$C" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/x"
echo '{"bomFormat":"CycloneDX","specVersion":"1.6","metadata":{"component":{"bom-ref":"v"}},"components":[]}' >"$t/vendor-empty.json"
expect_fail "vendor SBOM lacking pkg:npm components refused" "no pkg:npm" \
  "$S" -C "$C" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor-empty.json" -o "$t/x"
expect_fail "missing checkout refused" "not a directory" "$S" -C "$t/nope" -t aarch64-linux-android -o "$t/x"
[ ! -e "$t/x" ] || [ -z "$(ls -A "$t/x")" ] && ok "failed runs wrote no SBOM" || bad "a failed run left output in $t/x"

# --- lockfile + version catalog + inexact versions -------------------------------------------
L=$t/locked; mk_checkout "$L"
cat >"$L/src-tauri/gen/android/gradle/libs.versions.toml" <<'T'
[versions]
webkit = "1.14.0"
[libraries]
androidx-webkit = { module = "androidx.webkit:webkit", version.ref = "webkit" }
T
cat >"$L/src-tauri/gen/android/app/build.gradle.kts" <<'T'
android { defaultConfig { minSdk = 30 } }
dependencies {
    implementation(libs.androidx.webkit)
    implementation("com.example:dynamic:1.+")
}
T
cat >"$L/src-tauri/gen/android/app/gradle.lockfile" <<'T'
# This is a Gradle generated file for dependency locking.
androidx.webkit:webkit:1.14.0=releaseRuntimeClasspath
androidx.annotation:annotation:1.9.1=releaseRuntimeClasspath
empty=
T
"$S" -G -C "$L" >"$t/mvn.tsv"
awk -F'\t' '$1 == "androidx.annotation" && $5 == "true"' "$t/mvn.tsv" | grep -q . && ok "-G: lockfile entries are locked=true (transitive included)" || bad "-G lockfile: $(cat "$t/mvn.tsv")"
awk -F'\t' '$1 == "com.example" && $3 == "1.+" && $4 == "false" && $5 == "false"' "$t/mvn.tsv" | grep -q . && ok "-G: declared but unlocked dynamic version flagged" || bad "-G dynamic"
fl=$("$S" -C "$L" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/out3" 2>"$t/log3")
check "lockfile: annotation 1.9.1 locked purl" "$fl" \
  '[.components[] | select(.purl == "pkg:maven/androidx.annotation/annotation@1.9.1")][0] | any(.properties[]; .name == "opensidian:maven:locked" and .value == "true")'
check "dynamic 1.+: no purl, no invented version, flagged inexact" "$fl" \
  '[.components[] | select(.group == "com.example" and .name == "dynamic")][0] | .purl == null and .version == "1.+"
     and any(.properties[]; .name == "opensidian:maven:version-exact" and .value == "false")'
sed -i 's/libs.androidx.webkit/libs.androidx.missing/' "$L/src-tauri/gen/android/app/build.gradle.kts"
expect_fail "unknown version-catalog alias refused" "not in gradle/libs.versions.toml" \
  "$S" -C "$L" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/x"

# --- no Gradle project -----------------------------------------------------------------------
N=$t/nogradle; mk_checkout "$N"; rm -rf "$N/src-tauri/gen"
[ "$("$S" -L -C "$N")" = "aarch64-linux-android" ] && ok "-L without Gradle: aarch64 only" || bad "-L without gradle"
fn=$("$S" -C "$N" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/out4" 2>/dev/null)
check "no Gradle: no maven, WebView still listed" "$fn" \
  '([.components[] | select(.purl // "" | startswith("pkg:maven/"))] | length == 0) and any(.components[]; ."bom-ref" == "os:android-system-webview")'


# --- the android checkout is DATA: no cargo config of its own (CodeQL 968/969, cache poisoning) ---
# cargo metadata runs with cwd <checkout>/src-tauri, so cargo would honour <checkout>/.cargo/config*
# and <checkout>/src-tauri/.cargo/config* (build.rustc-wrapper, build.rustc, ...): code from the
# android branch running in the default-branch run. Every mode refuses such a checkout.
for p in .cargo/config.toml .cargo/config src-tauri/.cargo/config.toml src-tauri/.cargo/config; do
  K=$t/cfg-${p//\//_}; mk_checkout "$K"; mkdir -p "$K/$(dirname "$p")"
  printf '[build]\nrustc-wrapper = "/bin/false"\n' >"$K/$p"
  expect_fail "cargo config $p in the checkout refused (-L)" "cargo config" "$S" -L -C "$K"
  expect_fail "cargo config $p in the checkout refused (sbom)" "cargo config" \
    "$S" -C "$K" -t aarch64-linux-android -m "$t/meta.json" -j "$t/vendor.cdx.json" -o "$t/x"
done
K=$t/cfg-link; mk_checkout "$K"; mkdir -p "$t/elsewhere"; ln -s "$t/elsewhere" "$K/src-tauri/.cargo"
ln -s /dev/null "$t/elsewhere/config.toml"
expect_fail "cargo config reached through a symlinked .cargo refused" "cargo config" "$S" -L -C "$K"
K=$t/cfg-dangling; mk_checkout "$K"; mkdir -p "$K/.cargo"; ln -s "$t/nowhere" "$K/.cargo/config.toml"
expect_fail "dangling cargo config symlink refused" "cargo config" "$S" -L -C "$K"
K=$t/cfg-audit; mk_checkout "$K"; mkdir -p "$K/src-tauri/.cargo"; echo '[advisories]' >"$K/src-tauri/.cargo/audit.toml"
[ "$("$S" -L -C "$K")" = "$("$S" -L -C "$C")" ] && ok "a .cargo/audit.toml alone is not a cargo config" || bad "audit.toml refused"
[ "$fails" -eq 0 ] && echo "android-sbom.test: all passed" || { echo "android-sbom.test: FAILED"; exit 1; }
