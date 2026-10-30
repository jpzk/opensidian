// SPDX-License-Identifier: GPL-3.0-or-later
package dev.koto.opensidian

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  // M10.1 / M10.2: the system back (key or edge gesture) goes to the web UI's own layer stack
  // (ui/mobile.js window.__mobileBack: menu > modal > drawer > note history). Never WebView
  // history, never finish(): back at the root stays in the app.
  override val handleBackNavigation: Boolean = false

  private val ui = Handler(Looper.getMainLooper())
  private var safeJs = ""

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        webView.evaluateJavascript("window.__mobileBack&&window.__mobileBack()", null)
      }
    })
    // M2.2 / M3.1: the page is edge-to-edge; hand it the system-bar + cutout insets in css px
    // (device px / density) so the phone header and bottom bar pad themselves into the safe area.
    ViewCompat.setOnApplyWindowInsetsListener(webView) { v, insets ->
      val i = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
      val d = resources.displayMetrics.density
      safeJs = "window.__mobileSafe&&window.__mobileSafe(${i.top / d},${i.right / d},${i.bottom / d},${i.left / d})"
      pushSafe(webView)
      ViewCompat.onApplyWindowInsets(v, insets)
    }
  }

  // the first insets usually arrive before the page has loaded mobile.js: repeat the push a few
  // times over the cold start; every later change (rotation) pushes again
  private fun pushSafe(webView: WebView) {
    ui.removeCallbacksAndMessages(null)
    for (ms in longArrayOf(0, 300, 1000, 2500, 5000, 10000)) {
      ui.postDelayed({ if (safeJs.isNotEmpty()) webView.evaluateJavascript(safeJs, null) }, ms)
    }
  }
}
