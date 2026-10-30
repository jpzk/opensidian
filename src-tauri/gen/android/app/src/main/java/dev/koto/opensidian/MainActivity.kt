// SPDX-License-Identifier: GPL-3.0-or-later
package dev.koto.opensidian

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.DocumentsContract
import android.provider.Settings
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : TauriActivity() {
  // M10.1 / M10.2: the system back (key or edge gesture) goes to the web UI's own layer stack
  // (ui/mobile.js window.__mobileBack: menu > modal > drawer > note history). Never WebView
  // history, never finish(): back at the root stays in the app.
  override val handleBackNavigation: Boolean = false

  private val ui = Handler(Looper.getMainLooper())
  private var safeJs = ""
  private var web: WebView? = null

  // M1.2: "Open folder as vault" = the system folder picker (Storage Access Framework). The grant
  // is persisted; the tree is handed to the page as its filesystem path (the vault code is plain
  // file I/O), which the app can read once "All files access" is granted.
  private val pickTree = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri -> onTree(uri) }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    web = webView
    // the page only ever loads the app's own origin (navguard, lib.rs nav_allowed)
    webView.addJavascriptInterface(Bridge(), "__osAndroid")
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

  private fun js(s: String) = ui.post { web?.evaluateJavascript(s, null) }

  private fun onTree(uri: Uri?) {
    if (uri == null) { js("window.__mobileTreePicked&&window.__mobileTreePicked(null,null)"); return }
    try {
      contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
    } catch (e: SecurityException) {
      android.util.Log.w("opensidian", "tree grant not persisted: $e")
    }
    val path = treePath(uri)
    js("window.__mobileTreePicked&&window.__mobileTreePicked(${JSONObject.quote(uri.toString())},${if (path == null) "null" else JSONObject.quote(path)})")
  }

  // content://com.android.externalstorage.documents/tree/primary%3ADocuments%2Fv -> /storage/emulated/0/Documents/v
  // (another volume "XXXX-XXXX:dir" -> /storage/XXXX-XXXX/dir); any other provider has no path: null
  private fun treePath(uri: Uri): String? {
    if (uri.authority != "com.android.externalstorage.documents") return null
    val id = try { DocumentsContract.getTreeDocumentId(uri) } catch (e: IllegalArgumentException) { return null }
    val c = id.indexOf(':'); if (c < 0) return null
    val vol = id.substring(0, c); val rel = id.substring(c + 1).trim('/')
    if (rel.split('/').any { it == ".." || it == "." }) return null
    val root = if (vol == "primary") Environment.getExternalStorageDirectory().path else "/storage/$vol"
    return if (rel.isEmpty()) root else "$root/$rel"
  }

  private fun allFiles(): Boolean =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) Environment.isExternalStorageManager() else true

  inner class Bridge {
    @JavascriptInterface fun pickTree() { ui.post { pickTree.launch(null) } }
    @JavascriptInterface fun trees(): String =
      JSONArray(contentResolver.persistedUriPermissions.map { it.uri.toString() }).toString()
    @JavascriptInterface fun allFiles(): Boolean = this@MainActivity.allFiles()
    @JavascriptInterface fun requestAllFiles() {
      if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
      ui.post {
        startActivity(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:$packageName")))
      }
    }
    // M1.1/M1.2 vault switch: one process per vault, so a switch relaunches the app (Rust has
    // already recorded the new vault as the last one, android_boot binds it). The relaunch runs from
    // a helper activity in its own process (RestartActivity), so the new app process never shares
    // a life with the old one.
    @JavascriptInterface fun restart() {
      ui.post {
        startActivity(Intent(this@MainActivity, RestartActivity::class.java)
          .putExtra(RestartActivity.PID, android.os.Process.myPid())
          .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        finishAffinity()
        Runtime.getRuntime().exit(0)
      }
    }
  }
}

// ":restart" process: wait for the old app process to be gone, start the launcher activity in a
// fresh task, then leave
class RestartActivity : android.app.Activity() {
  companion object { const val PID = "pid" }
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val old = intent.getIntExtra(PID, -1)
    if (old > 0) android.os.Process.killProcess(old)
    packageManager.getLaunchIntentForPackage(packageName)?.let { startActivity(Intent.makeRestartActivityTask(it.component)) }
    finish()
    Runtime.getRuntime().exit(0)
  }
}
