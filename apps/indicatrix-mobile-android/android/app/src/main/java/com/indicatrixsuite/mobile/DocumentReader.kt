package com.indicatrixsuite.mobile

import android.content.ContentResolver
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import java.io.ByteArrayOutputStream
import java.io.IOException

/** The size limits for files that come from other apps. */
internal object Limits {
    /** One file. */
    const val MAX_FILE_BYTES: Long = 64L * 1024 * 1024

    /** Everything read for one request (a multi-pick, a folder, an incoming batch). */
    const val MAX_TOTAL_BYTES: Long = 256L * 1024 * 1024

    /** Design files collected from a picked folder. */
    const val MAX_TREE_FILES: Int = 2000
}

/** A limit above was exceeded; the message is shown to the user as it is. */
internal class ImportLimitException(message: String) : IOException(message)

/** Counts the bytes read for one request against [Limits.MAX_TOTAL_BYTES]. */
internal class ReadBudget {
    private var used: Long = 0

    fun take(bytes: Long) {
        used += bytes
        if (used > Limits.MAX_TOTAL_BYTES) {
            throw ImportLimitException("The selection is larger than 256 MiB in total. Pick fewer files.")
        }
    }
}

/** One design file found below a picked folder. */
internal class TreeEntry(val documentId: String, val name: String, val size: Long)

/** Reads files from `content:` and `file:` URIs, within the limits above. */
internal class DocumentReader(private val resolver: ContentResolver) {

    /** The name the user sees for [uri]; never empty. */
    fun displayName(uri: Uri): String {
        try {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val name = cursor.getString(0)
                    if (!name.isNullOrBlank()) return name
                }
            }
        } catch (e: RuntimeException) {
            // Fall through to the last path segment.
        }
        val segment = uri.lastPathSegment?.substringAfterLast('/')
        return if (segment.isNullOrBlank()) "Untitled" else segment
    }

    /**
     * Reads all of [uri]. Throws [ImportLimitException] when the file is over
     * [Limits.MAX_FILE_BYTES] or [budget] runs out, and [IOException] when it cannot be
     * opened. Never call this on the UI thread.
     */
    fun read(uri: Uri, name: String, budget: ReadBudget): ByteArray {
        val input = resolver.openInputStream(uri) ?: throw IOException("Could not open \"$name\".")
        return input.use { stream ->
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(64 * 1024)
            var total = 0L
            while (true) {
                val count = stream.read(buffer)
                if (count < 0) break
                total += count
                if (total > Limits.MAX_FILE_BYTES) {
                    throw ImportLimitException("\"$name\" is larger than 64 MiB and cannot be opened.")
                }
                out.write(buffer, 0, count)
            }
            budget.take(total)
            out.toByteArray()
        }
    }

    /**
     * Lists the design files below the folder [tree] (a document-tree URI), walking
     * sub-folders. Throws [ImportLimitException] past [Limits.MAX_TREE_FILES] files or
     * [Limits.MAX_TOTAL_BYTES] bytes.
     */
    fun listTree(tree: Uri): List<TreeEntry> {
        val found = ArrayList<TreeEntry>()
        var totalBytes = 0L
        val folders = ArrayList<String>()
        folders.add(DocumentsContract.getTreeDocumentId(tree))
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        while (folders.isNotEmpty()) {
            val parent = folders.removeAt(folders.lastIndex)
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent)
            resolver.query(children, projection, null, null, null)?.use { cursor ->
                while (cursor.moveToNext()) {
                    val id = cursor.getString(0)
                    val name = cursor.getString(1)
                    if (id == null || name == null) continue
                    if (cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                        folders.add(id)
                    } else if (isDesignFile(name)) {
                        val size = if (cursor.isNull(3)) 0L else cursor.getLong(3)
                        totalBytes += size
                        if (found.size >= Limits.MAX_TREE_FILES) {
                            throw ImportLimitException(
                                "The folder holds more than ${Limits.MAX_TREE_FILES} design files. Pick a smaller folder."
                            )
                        }
                        if (totalBytes > Limits.MAX_TOTAL_BYTES) {
                            throw ImportLimitException("The design files in the folder total more than 256 MiB. Pick a smaller folder.")
                        }
                        found.add(TreeEntry(id, name, size))
                    }
                }
            }
        }
        return found
    }

    private fun isDesignFile(name: String): Boolean {
        val lower = name.lowercase()
        return DESIGN_SUFFIXES.any { lower.endsWith(it) }
    }

    private companion object {
        /** Faceting designs the core can read (`.indicatrix` is the legacy design name). */
        val DESIGN_SUFFIXES = listOf(".isdd", ".indicatrix", ".asc", ".gem", ".gcs")
    }
}
