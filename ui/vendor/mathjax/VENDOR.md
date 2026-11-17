<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ui/vendor/mathjax — mathjax 3.2.2, vendored

Loaded on demand from 'self' by ui/plugins/api-functions.js (loadMathJax -> window.MathJax, renderMath, finishRenderMath); the app's own $..$ / $$..$$ rendering (ui/mathview.js, #185) uses the same loader. Unmodified upstream bytes;
no network, no eval, served under the existing CSP (script-src 'self').

- package: mathjax
- version: 3.2.2 (exact pin)
- published: 2022-06-08T17:48:38.006Z (npm registry `time["3.2.2"]`; 6-week rule: on or before 2026-09-06 for this pin)
- license: Apache-2.0 (upstream LICENSE copied beside it)
- tarball: https://registry.npmjs.org/mathjax/-/mathjax-3.2.2.tgz
- tarball integrity (npm dist.integrity, verified on download): `sha512-Bt+SSVU8eBG27zChVewOicYs7Xsdt40qm4+UpHyX7k0/O9NliPc+x77k1/FEsPsjKPZGJvtRZM1vO+geW0OhGw==`
- tarball sha256: `1b9c0a1c44df864e915690558e72adb9cc5203360daefd385084ced3b6c64c09`
- files taken from the tarball: package/es5/tex-chtml-full.js, package/es5/output/chtml/fonts/woff-v2/*.woff (the CHTML font files, same relative layout so the default fontURL resolves here), package/LICENSE

## sha256 (machine-checkable copy: SHA256SUMS)

Check: `cd ui/vendor/mathjax && sha256sum -c SHA256SUMS`

```
91b005503c5d1f0958bf4d73ada1a3be33596b2c4ab5da22277f1b0d3149b5a4  tex-chtml-full.js
cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30  LICENSE
3de784d07b9fa8f104c10928a878ee879cf3305cae5195cba663c9c2bb0195eb  output/chtml/fonts/woff-v2/MathJax_AMS-Regular.woff
af04542b29eaac04550a140c5f1760a649783989426f2540855bf4157819367d  output/chtml/fonts/woff-v2/MathJax_Calligraphic-Bold.woff
26683bf201fb258a2237d9754616de9d4ecf4cc1cd39dd1902476df7d75f1d16  output/chtml/fonts/woff-v2/MathJax_Calligraphic-Regular.woff
721921bab0d001ebff0206c24cb5de6ca136467bf1843a7aa32030ba061d1e92  output/chtml/fonts/woff-v2/MathJax_Fraktur-Bold.woff
870673df72e70f87c91a5a317d558c2c3b54392264ad79bbadc6d424ae8765fe  output/chtml/fonts/woff-v2/MathJax_Fraktur-Regular.woff
88b98cad3688915e50da55553ff6ad185e0dce134b47f176e91b100f8a9b175c  output/chtml/fonts/woff-v2/MathJax_Main-Bold.woff
355254db9ca10a09a3b5f0929d74eb4670f44fbe864c07526a06213e0a0caf6c  output/chtml/fonts/woff-v2/MathJax_Main-Italic.woff
1cb1c39ea642f26a4dfed230b4aea1c3c218689421f6e9c0a7c1811693c4fa07  output/chtml/fonts/woff-v2/MathJax_Main-Regular.woff
8ea8dbb1b02e6f730f55b4cb5d413b785b9f5c39807d0a0fa7da206a0824a457  output/chtml/fonts/woff-v2/MathJax_Math-BoldItalic.woff
a009bea404f7a500ded48f8b9ad9cf16e12504b3195dd9e25975289b8256b0f0  output/chtml/fonts/woff-v2/MathJax_Math-Italic.woff
c01d3321e89b403c4b811aa153c4e618eda3421f92d8a072a02c8d190782a191  output/chtml/fonts/woff-v2/MathJax_Math-Regular.woff
32792104b5ef69eded905b6d1598ed05d8087684d38e7a94d52e3c38ba16f47e  output/chtml/fonts/woff-v2/MathJax_SansSerif-Bold.woff
fc6ddf5df402b263cfb158aed8e89972542c34b719cd87b1db30461985f7bd5b  output/chtml/fonts/woff-v2/MathJax_SansSerif-Italic.woff
b418136e3b384baaadecb70bd3c48a8da9825210130b2897db808c44efc883cb  output/chtml/fonts/woff-v2/MathJax_SansSerif-Regular.woff
af96f67d7accf5fd2a4a682d0b9f8b339f8ea6fe34c310c1694c8ba7f6ddc96f  output/chtml/fonts/woff-v2/MathJax_Script-Regular.woff
c49810b53ecc0d87d8028762c518924197dd9d3f905b08f99ea241301085b9cb  output/chtml/fonts/woff-v2/MathJax_Size1-Regular.woff
30e889b58cbc51adfbb038ab1a96dc4025aa3542a3cff7712fc55ece510675e2  output/chtml/fonts/woff-v2/MathJax_Size2-Regular.woff
5cda41563a095bd70c78e2dda13d0f8cb922c71c90fffd0a0044c65173a66e83  output/chtml/fonts/woff-v2/MathJax_Size3-Regular.woff
3bc6ecaae7ecf6f8d7f8ea07bddbaca8601e2cb6e89d6aef58e78d9f6d8a398f  output/chtml/fonts/woff-v2/MathJax_Size4-Regular.woff
c56da8d69f1a0208b8e0703656c3264b6dd748bd452524c82b0385b60f6a68c1  output/chtml/fonts/woff-v2/MathJax_Typewriter-Regular.woff
36e0d72d8a7afc696a3e7a5c7369807634640de9b02257bca447bdf126221a27  output/chtml/fonts/woff-v2/MathJax_Vector-Bold.woff
72bc573386dd1d48c5bbd286302ca9e3400c5eb0f298e7cfbf945b9d08fc688f  output/chtml/fonts/woff-v2/MathJax_Vector-Regular.woff
481e39042508ae313a60618af1e37146ab93e9324c98e4c78b8f17fe55d41e0b  output/chtml/fonts/woff-v2/MathJax_Zero.woff
```
