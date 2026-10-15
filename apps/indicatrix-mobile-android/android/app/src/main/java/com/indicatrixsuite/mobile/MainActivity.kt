package com.indicatrixsuite.mobile

import android.app.NativeActivity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Looper
import android.os.PowerManager
import android.print.PrintManager
import android.provider.DocumentsContract
import android.util.Log
import android.view.WindowManager
import android.webkit.MimeTypeMap
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import java.io.File
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * The app's only activity: a [NativeActivity] that runs the Rust/Slint application and
 * provides the Android services it cannot reach itself.
 *
 * Rust calls the public methods below by name over JNI (`platform_android/mod.rs`), and
 * answers to asynchronous requests come back through the `native*` functions (declared
 * at the bottom, implemented in `platform_android/natives.rs`). Keep both sides in step.
 *
 * Convention: every service method returns `null` on success and a sentence for the
 * user on failure. `thermalHeadroom` and `cacheDirPath` return their value.
 *
 * NativeActivity is not a ComponentActivity, so results come through
 * `startActivityForResult` / `onActivityResult` with request codes mapped to the Rust
 * request ids.
 */
@Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
class MainActivity : NativeActivity() {

    /** A system dialog we started and are waiting for. UI thread only. */
    private class PendingRequest(
        val rustId: Long,
        val kind: Int,
        val saveName: String? = null,
        val saveBytes: ByteArray? = null,
    )

    private val requests = HashMap<Int, PendingRequest>()
    private var nextRequestCode = FIRST_REQUEST_CODE

    /** The WebView of the print job in flight; the print adapter needs it alive. */
    private var printView: WebView? = null

    // ---------------------------------------------------------------------------------
    // Lifecycle
    // ---------------------------------------------------------------------------------

    override fun onCreate(savedInstanceState: Bundle?) {
        // Edge to edge: Slint reads the insets and keeps the UI out of the system bars.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        super.onCreate(savedInstanceState)
        // A recreated activity gets the same launch intent again; import it only once.
        if (savedInstanceState == null) {
            handleIncoming(intent)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncoming(intent)
    }

    override fun onDestroy() {
        printView = null
        super.onDestroy()
    }

    // ---------------------------------------------------------------------------------
    // Services Rust calls
    // ---------------------------------------------------------------------------------

    /** The app's cache directory (`Context.getCacheDir()`). */
    fun cacheDirPath(): String = cacheDir.absolutePath

    /**
     * Opens the system picker. [kind]: 0 designs, 1 HDR map, 2 design folder. The
     * answer arrives as `nativeFileReady` per file, then `nativeRequestDone`.
     */
    fun openDocuments(requestId: Long, kind: Int, allowMultiple: Boolean): String? {
        runOnUiThread {
            try {
                val intent = if (kind == KIND_DESIGN_FOLDER) {
                    Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                } else {
                    Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                        addCategory(Intent.CATEGORY_OPENABLE)
                        // Design and HDR files rarely carry a reliable MIME type, so the
                        // picker is not narrowed; the core classifies by content.
                        type = "*/*"
                        if (allowMultiple && kind == KIND_DESIGNS) {
                            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
                        }
                    }
                }
                startForResult(intent, PendingRequest(requestId, kind))
            } catch (e: Exception) {
                Log.w(TAG, "openDocuments failed", e)
                nativeRequestDone(requestId, describe(e))
            }
        }
        return null
    }

    /**
     * Opens the system "save as" dialog and writes [bytes] where the user chooses. The
     * answer arrives as `nativeSaveDone`.
     */
    fun createDocument(requestId: Long, name: String, mime: String, bytes: ByteArray): String? {
        runOnUiThread {
            try {
                // The dialog appends an extension for the MIME type it is given ("x.asc"
                // would become "x.asc.txt" for text/plain), so a type that does not match
                // the file name's extension is replaced by the generic one.
                val extension = name.substringAfterLast('.', "").lowercase()
                val known = MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
                val pickerType = if (known == mime) mime else "application/octet-stream"
                val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE)
                    type = pickerType
                    putExtra(Intent.EXTRA_TITLE, name)
                }
                startForResult(intent, PendingRequest(requestId, KIND_SAVE, name, bytes))
            } catch (e: Exception) {
                Log.w(TAG, "createDocument failed", e)
                nativeSaveDone(requestId, null, describe(e))
            }
        }
        return null
    }

    /** Offers a copy of the file to other apps through the share sheet. */
    fun share(name: String, mime: String, bytes: ByteArray): String? {
        val file = try {
            val dir = File(cacheDir, SHARE_DIR)
            dir.mkdirs()
            // Only the latest export is needed; keep the cache small.
            dir.listFiles()?.forEach { it.delete() }
            File(dir, safeFileName(name)).also { it.writeBytes(bytes) }
        } catch (e: Exception) {
            Log.w(TAG, "share: could not write the export", e)
            return describe(e)
        }
        return onUiThread {
            val uri = FileProvider.getUriForFile(this, "$packageName.files", file)
            val send = Intent(Intent.ACTION_SEND).apply {
                type = mime
                putExtra(Intent.EXTRA_STREAM, uri)
                clipData = ClipData.newRawUri(name, uri)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            val chooser = Intent.createChooser(send, null)
            chooser.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            startActivity(chooser)
            null
        }
    }

    /** Opens the system print dialog for an HTML document. */
    fun printHtml(title: String, html: String): String? = onUiThread {
        val webView = WebView(this)
        webView.settings.javaScriptEnabled = false
        webView.settings.allowFileAccess = false
        webView.webViewClient = object : WebViewClient() {
            private var started = false

            override fun onPageFinished(view: WebView, url: String?) {
                if (started) return
                started = true
                try {
                    val manager = getSystemService(Context.PRINT_SERVICE) as PrintManager
                    manager.print(title, view.createPrintDocumentAdapter(title), null)
                } catch (e: Exception) {
                    Log.w(TAG, "printHtml: could not start the print job", e)
                }
            }
        }
        // The print adapter reads from this view after the call returns: keep it alive
        // until the next print job or the end of the activity.
        printView = webView
        webView.loadDataWithBaseURL(null, html, "text/html", "UTF-8", null)
        null
    }

    /** Puts [text] on the clipboard. */
    fun setClipboardText(text: String): String? = onUiThread {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard.setPrimaryClip(ClipData.newPlainText("Indicatrix", text))
        null
    }

    /** Keeps the screen on (or stops doing so). Fire and forget. */
    fun keepScreenOn(on: Boolean): String? {
        runOnUiThread {
            if (on) {
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
        return null
    }

    /**
     * The thermal headroom forecast ten seconds ahead (0 cool, 1 throttling threshold),
     * or NaN where the device does not report it (Android below 11).
     */
    fun thermalHeadroom(): Float {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return Float.NaN
        return try {
            (getSystemService(Context.POWER_SERVICE) as PowerManager).getThermalHeadroom(10)
        } catch (e: Exception) {
            Float.NaN
        }
    }

    /** Opens a web address in the browser. */
    fun openUrl(url: String): String? = onUiThread {
        val uri = Uri.parse(url)
        val scheme = uri.scheme?.lowercase()
        if (scheme != "https" && scheme != "http") {
            return@onUiThread "Only web addresses (http or https) can be opened."
        }
        startActivity(Intent(Intent.ACTION_VIEW, uri))
        null
    }

    // ---------------------------------------------------------------------------------
    // System dialog results
    // ---------------------------------------------------------------------------------

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        val request = requests.remove(requestCode) ?: return
        if (request.kind == KIND_SAVE) {
            finishSave(request, resultCode, data)
        } else {
            finishPick(request, resultCode, data)
        }
    }

    private fun startForResult(intent: Intent, request: PendingRequest) {
        val code = nextRequestCode++
        requests[code] = request
        try {
            startActivityForResult(intent, code)
        } catch (e: Exception) {
            requests.remove(code)
            throw e
        }
    }

    /** Reads the picked files on a background thread and hands them to Rust. */
    private fun finishPick(request: PendingRequest, resultCode: Int, data: Intent?) {
        val id = request.rustId
        if (resultCode != RESULT_OK || data == null) {
            nativeRequestDone(id, null) // cancelled: no files
            return
        }
        thread(name = "indicatrix-pick") {
            val error = try {
                val reader = DocumentReader(contentResolver)
                val budget = ReadBudget()
                if (request.kind == KIND_DESIGN_FOLDER) {
                    data.data?.let { deliverTree(id, reader, budget, it) }
                } else {
                    for (uri in urisOf(data)) {
                        deliver(id, reader, budget, uri, reader.displayName(uri))
                    }
                }
                null
            } catch (e: Exception) {
                Log.w(TAG, "reading the picked files failed", e)
                describe(e)
            }
            nativeRequestDone(id, error)
        }
    }

    /** Writes the bytes of a save request to the chosen document. */
    private fun finishSave(request: PendingRequest, resultCode: Int, data: Intent?) {
        val id = request.rustId
        val uri = data?.data
        val bytes = request.saveBytes
        if (resultCode != RESULT_OK || uri == null || bytes == null) {
            nativeSaveDone(id, null, null) // cancelled
            return
        }
        thread(name = "indicatrix-save") {
            try {
                val stream = contentResolver.openOutputStream(uri, "wt")
                    ?: throw IOException("Could not open the file for writing.")
                stream.use { it.write(bytes) }
                nativeSaveDone(id, DocumentReader(contentResolver).displayName(uri), null)
            } catch (e: Exception) {
                Log.w(TAG, "saving the document failed", e)
                nativeSaveDone(id, null, describe(e))
            }
        }
    }

    private fun deliverTree(id: Long, reader: DocumentReader, budget: ReadBudget, tree: Uri) {
        for (entry in reader.listTree(tree)) {
            val uri = DocumentsContract.buildDocumentUriUsingTree(tree, entry.documentId)
            deliver(id, reader, budget, uri, entry.name)
        }
    }

    private fun deliver(id: Long, reader: DocumentReader, budget: ReadBudget, uri: Uri, name: String) {
        nativeFileReady(id, name, reader.read(uri, name, budget))
    }

    /** Every URI in a picker result: the single `data` and each `clipData` item. */
    private fun urisOf(intent: Intent): List<Uri> {
        val uris = LinkedHashSet<Uri>()
        intent.data?.let { uris.add(it) }
        val clip = intent.clipData
        if (clip != null) {
            for (i in 0 until clip.itemCount) {
                clip.getItemAt(i).uri?.let { uris.add(it) }
            }
        }
        return uris.toList()
    }

    // ---------------------------------------------------------------------------------
    // "Open with" and "Share to"
    // ---------------------------------------------------------------------------------

    /**
     * Reads the files another app sent (VIEW, SEND, SEND_MULTIPLE) and delivers them to
     * Rust as one batch on request id 0. A file that cannot be read is skipped.
     */
    private fun handleIncoming(intent: Intent?) {
        if (intent == null) return
        val uris: List<Uri> = when (intent.action) {
            Intent.ACTION_VIEW -> listOfNotNull(intent.data)
            Intent.ACTION_SEND -> listOfNotNull(streamExtra(intent))
            Intent.ACTION_SEND_MULTIPLE -> streamListExtra(intent)
            else -> emptyList()
        }
        if (uris.isEmpty()) return
        thread(name = "indicatrix-incoming") {
            val reader = DocumentReader(contentResolver)
            val budget = ReadBudget()
            for (uri in uris) {
                try {
                    deliver(INCOMING_ID, reader, budget, uri, reader.displayName(uri))
                } catch (e: Exception) {
                    Log.w(TAG, "an incoming file could not be read", e)
                }
            }
            nativeRequestDone(INCOMING_ID, null)
        }
    }

    private fun streamExtra(intent: Intent): Uri? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
        }

    private fun streamListExtra(intent: Intent): List<Uri> {
        val list = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
        }
        return list?.toList() ?: emptyList()
    }

    // ---------------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------------

    /**
     * Runs [action] on the UI thread and returns its result, waiting at most two
     * seconds. The wait is bounded because the UI thread can itself be waiting for the
     * native thread (lifecycle callbacks), which must not deadlock; on a timeout the
     * action still runs, its result is just not reported.
     */
    private fun onUiThread(action: () -> String?): String? {
        if (Looper.myLooper() == Looper.getMainLooper()) return guarded(action)
        val done = CountDownLatch(1)
        var result: String? = null
        runOnUiThread {
            result = guarded(action)
            done.countDown()
        }
        return if (done.await(UI_WAIT_MS, TimeUnit.MILLISECONDS)) result else null
    }

    private fun guarded(action: () -> String?): String? =
        try {
            action()
        } catch (e: Exception) {
            Log.w(TAG, "a platform service failed", e)
            describe(e)
        }

    /** A sentence for the user for [e]. */
    private fun describe(e: Exception): String =
        when {
            e is ActivityNotFoundException -> "No app is available to do this."
            e is ImportLimitException -> e.message ?: "The file is too large."
            e.message.isNullOrBlank() -> e.javaClass.simpleName
            else -> e.message ?: e.javaClass.simpleName
        }

    /** A file name that is safe inside the share directory. */
    private fun safeFileName(name: String): String {
        val cleaned = name.map { if (it.isLetterOrDigit() || it in "._- ") it else '_' }
            .joinToString("")
            .trim()
            .trimStart('.')
            .take(100)
        return cleaned.ifEmpty { "export" }
    }

    // ---------------------------------------------------------------------------------
    // Rust (platform_android/natives.rs)
    // ---------------------------------------------------------------------------------

    private external fun nativeFileReady(requestId: Long, name: String, bytes: ByteArray)

    private external fun nativeRequestDone(requestId: Long, error: String?)

    private external fun nativeSaveDone(requestId: Long, displayName: String?, error: String?)

    companion object {
        private const val TAG = "indicatrix"

        // `openDocuments` kinds; the numbers are shared with Rust (platform_android/mod.rs).
        private const val KIND_DESIGNS = 0
        private const val KIND_DESIGN_FOLDER = 2

        /** Request kind for a save dialog (never sent from Rust). */
        private const val KIND_SAVE = 100

        /** Rust's id for files nobody asked for. */
        private const val INCOMING_ID = 0L

        private const val FIRST_REQUEST_CODE = 0x1000
        private const val UI_WAIT_MS = 2000L
        private const val SHARE_DIR = "share"

        init {
            System.loadLibrary("indicatrix_mobile_android")
        }
    }
}
