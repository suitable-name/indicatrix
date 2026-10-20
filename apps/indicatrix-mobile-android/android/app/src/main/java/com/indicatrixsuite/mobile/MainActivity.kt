package com.indicatrixsuite.mobile

import android.app.NativeActivity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.net.ConnectivityManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.print.PrintAttributes
import android.print.PrintManager
import android.provider.DocumentsContract
import android.util.Log
import android.view.WindowManager
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.annotation.RequiresApi
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import java.io.File
import java.io.IOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
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
        val saveBytes: ByteArray? = null,
    )

    private val requests = HashMap<Int, PendingRequest>()
    private var nextRequestCode = FIRST_REQUEST_CODE

    /** The WebView of the print job in flight; the print adapter needs it alive. */
    private var printView: WebView? = null
    private var backNavigation: BackNavigationController? = null

    // ---------------------------------------------------------------------------------
    // Lifecycle
    // ---------------------------------------------------------------------------------

    override fun onCreate(savedInstanceState: Bundle?) {
        // Edge to edge: Slint reads the insets and keeps the UI out of the system bars.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 33) {
            backNavigation = BackNavigationController(this, this::nativeBackCommitted, this::nativeBackProgress)
        }
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

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    override fun onDestroy() {
        backNavigation?.close()
        backNavigation = null
        printView = null
        super.onDestroy()
    }

    // ---------------------------------------------------------------------------------
    // Services Rust calls
    // ---------------------------------------------------------------------------------
    // Every method in this section is called only from Rust over JNI (`platform_android`);
    // Kotlin sees no callers, so never remove one as unused. JNI calls arrive on a
    // non-UI thread: UI work hops to the UI thread through [onUi].

    private val vault by lazy { RemoteCredentialVault(this) }

    /** `openDocuments(JIZ)Ljava/lang/String;`: starts the system picker for [kind]. */
    fun openDocuments(requestId: Long, kind: Int, allowMultiple: Boolean): String? = guarded {
        val intent = when (kind) {
            KIND_DESIGN_FOLDER -> Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            KIND_DESIGNS, KIND_HDR_MAP -> Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "*/*" // design and .hdr files have no reliable MIME type
                putExtra(Intent.EXTRA_ALLOW_MULTIPLE, allowMultiple && kind == KIND_DESIGNS)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            else -> throw IllegalArgumentException("Unknown request kind $kind.")
        }
        startForResult(intent, requestId, kind, null)
        null
    }

    /** `createDocument(JLjava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;`: save dialog; the bytes wait in the request. */
    fun createDocument(requestId: Long, name: String, mime: String, bytes: ByteArray): String? = guarded {
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = mime.ifBlank { "application/octet-stream" }
            putExtra(Intent.EXTRA_TITLE, name)
        }
        startForResult(intent, requestId, KIND_SAVE, bytes)
        null
    }

    /** `share(Ljava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;`: share sheet for a copy in cache/share/. */
    fun share(name: String, mime: String, bytes: ByteArray): String? = guarded {
        val dir = File(cacheDir, "share")
        if (!dir.isDirectory && !dir.mkdirs()) throw IOException("Could not prepare the share folder.")
        val file = File(dir, safeFileName(name))
        dir.listFiles()?.forEach { if (it != file) it.delete() }
        file.writeBytes(bytes)
        val uri = FileProvider.getUriForFile(this, FILE_AUTHORITY, file)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = mime.ifBlank { "application/octet-stream" }
            putExtra(Intent.EXTRA_STREAM, uri)
            clipData = ClipData.newRawUri(file.name, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val chooser = Intent.createChooser(send, null).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        onUi { startActivity(chooser) }
        null
    }

    /** `printHtml(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;`: loads [html] in a WebView, then opens the print dialog. */
    fun printHtml(title: String, html: String): String? = guarded {
        val jobName = title.ifBlank { "Indicatrix" }
        onUi {
            val view = WebView(this)
            view.settings.javaScriptEnabled = false
            view.settings.allowFileAccess = false
            view.settings.allowContentAccess = false
            var started = false
            view.webViewClient = object : WebViewClient() {
                override fun onPageFinished(page: WebView, url: String?) {
                    if (started) return
                    started = true
                    try {
                        val manager = getSystemService(PrintManager::class.java)
                        manager.print(jobName, page.createPrintDocumentAdapter(jobName), PrintAttributes.Builder().build())
                    } catch (e: Exception) {
                        Log.w(TAG, "starting the print job failed", e)
                    }
                }
            }
            printView = view // the print adapter needs the view alive
            view.loadDataWithBaseURL(null, html, "text/html", "utf-8", null)
        }
        null
    }

    /** `setClipboardText(Ljava/lang/String;)Ljava/lang/String;`: replaces the primary clip with plain text. */
    fun setClipboardText(text: String): String? = guarded {
        onUi {
            val clipboard = getSystemService(ClipboardManager::class.java)
            clipboard.setPrimaryClip(ClipData.newPlainText("Indicatrix", text))
        }
        null
    }

    /** `keepScreenOn(Z)Ljava/lang/String;`: sets or clears the keep-screen-on window flag. */
    fun keepScreenOn(on: Boolean): String? = guarded {
        onUi {
            if (on) {
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
        null
    }

    /** `thermalHeadroom()F`: the current thermal headroom, or NaN where the device does not report it. */
    fun thermalHeadroom(): Float =
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                getSystemService(PowerManager::class.java).getThermalHeadroom(0)
            } else {
                Float.NaN
            }
        } catch (e: Exception) {
            Log.w(TAG, "thermal headroom is unavailable", e)
            Float.NaN
        }

    /** `openUrl(Ljava/lang/String;)Ljava/lang/String;`: opens a web link in the browser. */
    fun openUrl(url: String): String? = guarded {
        val uri = Uri.parse(url)
        if (uri.scheme != "https" && uri.scheme != "http") {
            throw IllegalArgumentException("Only web links can be opened.")
        }
        onUi { startActivity(Intent(Intent.ACTION_VIEW, uri)) }
        null
    }

    /** `cacheDirPath()Ljava/lang/String;`: the app's cache directory. */
    fun cacheDirPath(): String = cacheDir.absolutePath

    /** `systemPowerSave()Z`: whether the battery saver is on. */
    fun systemPowerSave(): Boolean =
        try {
            getSystemService(PowerManager::class.java).isPowerSaveMode
        } catch (e: Exception) {
            false
        }

    /** `networkMetered()I`: 1 metered, 0 unmetered, -1 unknown. */
    fun networkMetered(): Int =
        try {
            val manager = getSystemService(ConnectivityManager::class.java)
            when {
                manager.activeNetwork == null -> -1
                manager.isActiveNetworkMetered -> 1
                else -> 0
            }
        } catch (e: Exception) {
            -1
        }

    /** `setBackTarget(II)V`: mirrors Rust's Back state into the [BackNavigationController]. */
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    fun setBackTarget(target: Int, version: Int) {
        runOnUiThread { backNavigation?.setTarget(target, version) }
    }

    /** `storeRemoteCredentials(Ljava/lang/String;)Ljava/lang/String;`: Keystore-wrapped store. */
    fun storeRemoteCredentials(bundle: String): String? = guarded {
        vault.store(bundle)
        null
    }

    /** `loadRemoteCredentials()Ljava/lang/String;`: the bundle, `null` if none; an exception escapes if it cannot be unlocked. */
    fun loadRemoteCredentials(): String? = vault.load()

    /** `forgetRemoteCredentials()Ljava/lang/String;`: deletes the encrypted file and the Keystore key. */
    fun forgetRemoteCredentials(): String? = guarded {
        vault.forget()
        null
    }

    /** Registers a [PendingRequest] under a fresh request code and starts [intent] on the UI thread. */
    private fun startForResult(intent: Intent, rustId: Long, kind: Int, saveBytes: ByteArray?) {
        onUi {
            val code = nextRequestCode++
            requests[code] = PendingRequest(rustId, kind, saveBytes)
            try {
                startActivityForResult(intent, code)
            } catch (e: Exception) {
                requests.remove(code)
                throw e
            }
        }
    }

    /** Runs [block] on the UI thread, waits up to [UI_WAIT_MS] and rethrows what it threw. */
    private fun <R> onUi(block: () -> R): R {
        val result = AtomicReference<Any?>()
        val failure = AtomicReference<Throwable?>()
        val done = CountDownLatch(1)
        runOnUiThread {
            try {
                result.set(block())
            } catch (t: Throwable) {
                failure.set(t)
            } finally {
                done.countDown()
            }
        }
        if (!done.await(UI_WAIT_MS, TimeUnit.MILLISECONDS)) {
            throw IOException("The app did not respond in time.")
        }
        failure.get()?.let { throw it }
        @Suppress("UNCHECKED_CAST")
        return result.get() as R
    }

    /** A file name safe inside cache/share/: no separators, no leading dots. */
    private fun safeFileName(name: String): String {
        val base = name.substringAfterLast('/').substringAfterLast('\\')
        val cleaned = base.replace(Regex("[^A-Za-z0-9._ -]"), "_").trimStart('.').trim().take(100)
        return cleaned.ifEmpty { "export" }
    }

    private external fun nativeBackCommitted(target: Int, version: Int)
    private external fun nativeBackProgress(target: Int, version: Int, progress: Float)

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

    /** Reads the picked files on a background thread and hands them to Rust. */
    private fun finishPick(request: PendingRequest, resultCode: Int, data: Intent?) {
        val id = request.rustId
        if (resultCode != RESULT_OK || data == null) {
            nativeRequestDone(id, null) // canceled: no files
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
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        }

    private fun streamListExtra(intent: Intent): List<Uri> {
        val list = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
        }
        return list?.toList() ?: emptyList()
    }

    // ---------------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------------

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

    // ---------------------------------------------------------------------------------
    // Rust (platform_android/natives.rs)
    // ---------------------------------------------------------------------------------

    private external fun nativeFileReady(requestId: Long, name: String, bytes: ByteArray)

    private external fun nativeRequestDone(requestId: Long, error: String?)

    private external fun nativeSaveDone(requestId: Long, displayName: String?, error: String?)

    companion object {
        private const val TAG = "indicatrix"

        /** Request kinds sent from Rust (`KIND_*` in platform_android/mod.rs). */
        private const val KIND_DESIGNS = 0
        private const val KIND_HDR_MAP = 1
        private const val KIND_DESIGN_FOLDER = 2

        /** The FileProvider authority declared in the manifest. */
        private const val FILE_AUTHORITY = "com.indicatrixsuite.mobile.files"

        /** Request kind for a save dialog (never sent from Rust). */
        private const val KIND_SAVE = 100

        /** Rust's id for files nobody asked for. */
        private const val INCOMING_ID = 0L

        private const val FIRST_REQUEST_CODE = 0x1000
        private const val UI_WAIT_MS = 2000L

        init {
            System.loadLibrary("indicatrix_mobile_android")
        }
    }
}
