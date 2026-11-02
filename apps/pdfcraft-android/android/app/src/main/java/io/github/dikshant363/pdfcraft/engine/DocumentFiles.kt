package io.github.dikshant363.pdfcraft.engine

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import androidx.core.content.FileProvider
import io.github.dikshant363.pdfcraft.R
import java.io.File
import java.io.IOException
import java.security.MessageDigest

/**
 * Where the app keeps the files it works on. The engine reads and writes real paths, so a
 * document opened from another app (a `content://` URI) is copied into the app's files first, and
 * saving writes the result back to that URI when the app may write there.
 */
class DocumentFiles(private val context: Context) {
    private val resolver: ContentResolver get() = context.contentResolver

    /** Working copies of opened documents, one folder per source. */
    private val workDir = File(context.filesDir, "Documents").apply { mkdirs() }

    /** Files handed to other apps (share, print), through the FileProvider. */
    private val shareDir = File(context.cacheDir, "share").apply { mkdirs() }

    /** Files the app creates itself (combine, create, scans) before the user saves them somewhere. */
    val createdDir = File(context.filesDir, "Created").apply { mkdirs() }

    /** The working copy of [uri]: copied in (again) unless it is a file the app already owns. */
    fun workingCopy(uri: Uri): File {
        if (uri.scheme == ContentResolver.SCHEME_FILE) {
            val f = File(requireNotNull(uri.path) { context.getString(R.string.files_error_no_path) })
            if (f.canonicalPath.startsWith(context.filesDir.canonicalPath)) return f
        }
        val name = displayName(uri)
        val dir = File(workDir, sha(uri.toString())).apply { mkdirs() }
        val target = File(dir, name)
        val tmp = File(dir, ".$name.part")
        resolver.openInputStream(uri).use { input ->
            requireNotNull(input) { context.getString(R.string.common_file_unreadable) }
            tmp.outputStream().use { input.copyTo(it) }
        }
        if (!tmp.renameTo(target)) {
            tmp.copyTo(target, overwrite = true)
            tmp.delete()
        }
        // the copy keeps the source's date, which the More sheet shows as when it was modified
        lastModified(uri)?.let { target.setLastModified(it) }
        return target
    }

    /** When [uri] was last modified (ms), when its provider says: a document's or a media file's date. */
    fun lastModified(uri: Uri): Long? {
        if (uri.scheme == ContentResolver.SCHEME_FILE) return uri.path?.let { File(it).lastModified() }?.takeIf { it > 0 }
        fun column(name: String): Long? = runCatching {
            resolver.query(uri, arrayOf(name), null, null, null)?.use { c -> if (c.moveToFirst() && !c.isNull(0)) c.getLong(0) else null }
        }.getOrNull()
        return column(DocumentsContract.Document.COLUMN_LAST_MODIFIED)?.takeIf { it > 0 }
            ?: column(MediaStore.MediaColumns.DATE_MODIFIED)?.takeIf { it > 0 }?.times(1000)
    }

    /** The working copy made earlier for [uri], if there is one (for thumbnails). */
    fun existingCopy(uri: Uri): File? =
        File(workDir, sha(uri.toString())).listFiles()?.firstOrNull { it.isFile && !it.name.startsWith(".") }

    /** The file name another app gives [uri], ending in `.pdf`. */
    fun displayName(uri: Uri): String {
        val name = runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(0) else null
            }
        }.getOrNull() ?: uri.lastPathSegment?.substringAfterLast('/') ?: "Document.pdf"
        return safeName(name)
    }

    /** The file name another app gives [uri], as it is (any extension). */
    fun displayNameRaw(uri: Uri): String {
        val name = runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
        }.getOrNull() ?: uri.lastPathSegment?.substringAfterLast('/') ?: "File"
        return name.replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_").trim().ifEmpty { "File" }
    }

    /** The size of [uri] in bytes, when its provider says. */
    fun size(uri: Uri): Long? {
        if (uri.scheme == ContentResolver.SCHEME_FILE) return uri.path?.let { File(it).length() }
        return runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use { c -> if (c.moveToFirst() && !c.isNull(0)) c.getLong(0) else null }
        }.getOrNull()
    }

    /** Whether the app may write to [uri] (a document the user picked with write access). */
    fun canWrite(uri: Uri): Boolean {
        if (uri.scheme == ContentResolver.SCHEME_FILE) return uri.path?.let { File(it).canWrite() } == true
        // a provider that says its document can't be written refuses whatever access the picker
        // granted (Android's media documents, which the picker's Recent shows: "Media is read-only")
        if (DocumentsContract.isDocumentUri(context, uri) && documentFlags(uri)?.let { it and DocumentsContract.Document.FLAG_SUPPORTS_WRITE == 0 } == true) return false
        val granted = resolver.persistedUriPermissions.any { it.uri == uri && it.isWritePermission }
        return granted || runCatching { resolver.openFileDescriptor(uri, "wa")?.close(); true }.getOrDefault(false)
    }

    /** A document's flags (DocumentsContract.Document.FLAG_*), when its provider says. */
    private fun documentFlags(uri: Uri): Int? = runCatching {
        resolver.query(uri, arrayOf(DocumentsContract.Document.COLUMN_FLAGS), null, null, null)?.use { c -> if (c.moveToFirst() && !c.isNull(0)) c.getInt(0) else null }
    }.getOrNull()

    /** Write [file] over [uri] (truncating it). */
    fun writeBack(file: File, uri: Uri) {
        val out = resolver.openOutputStream(uri, "wt") ?: error(context.getString(R.string.files_error_write))
        out.use { o -> file.inputStream().use { it.copyTo(o) } }
    }

    /** Keep access to [uri] across restarts, for Recent files (when the provider allows it). */
    fun keepAccess(uri: Uri) {
        if (uri.scheme != ContentResolver.SCHEME_CONTENT) return
        runCatching { resolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION) }
            .recoverCatching { resolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
    }

    /** A copy of [file] under [name] that other apps may read, and its URI. */
    fun shareable(file: File, name: String = file.name): Uri = shareableAll(listOf(file to safeName(name))).first()

    /**
     * Copies of [files] that other apps may read (a document, pictures of pages), each under its
     * own name, and their URIs. The last ones handed out are cleared first. When the copies can't
     * be made (a full phone has no room for them), the error says so in words.
     */
    fun shareableAll(files: List<Pair<File, String>>): List<Uri> {
        shareDir.listFiles()?.forEach { it.deleteRecursively() }
        val dir = File(shareDir, System.nanoTime().toString())
        try {
            if (!dir.mkdirs()) throw IOException("can't make $dir")
            return files.map { (file, name) ->
                val copy = File(dir, name.replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_").ifBlank { file.name })
                file.copyTo(copy, overwrite = true)
                FileProvider.getUriForFile(context, "${context.packageName}.files", copy)
            }
        } catch (e: IOException) {
            dir.deleteRecursively()
            val full = shareDir.usableSpace < files.sumOf { it.first.length() } + 1024 * 1024
            throw IOException(context.getString(if (full) R.string.files_error_share_no_space else R.string.files_error_share), e)
        }
    }

    /** A new file in the app's own space for a document it makes, named [name] (made unique). */
    fun newCreated(name: String): File {
        val base = safeName(name).removeSuffix(".pdf")
        var f = File(createdDir, "$base.pdf")
        var n = 1
        while (f.exists()) f = File(createdDir, "$base (${n++}).pdf")
        return f
    }

    /**
     * Save [file] into the phone's Downloads as [name] (made unique by the system if taken) and
     * return its URI; no storage permission is needed for the app's own downloads.
     */
    fun saveToDownloads(file: File, name: String, mime: String = "application/pdf"): Uri {
        val values = android.content.ContentValues().apply {
            put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(android.provider.MediaStore.MediaColumns.MIME_TYPE, mime)
            put(android.provider.MediaStore.MediaColumns.RELATIVE_PATH, android.os.Environment.DIRECTORY_DOWNLOADS)
            put(android.provider.MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val collection = android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI
        val uri = resolver.insert(collection, values) ?: error(context.getString(R.string.files_error_save_downloads))
        try {
            resolver.openOutputStream(uri).use { out ->
                requireNotNull(out) { context.getString(R.string.files_error_save_downloads) }
                file.inputStream().use { it.copyTo(out) }
            }
            resolver.update(uri, android.content.ContentValues().apply { put(android.provider.MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
        } catch (e: Exception) {
            resolver.delete(uri, null, null)
            throw e
        }
        return uri
    }

    /** Save an image into Pictures/PdfCraft (exported pages). */
    fun saveToPictures(file: File, name: String, mime: String): Uri {
        val values = android.content.ContentValues().apply {
            put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(android.provider.MediaStore.MediaColumns.MIME_TYPE, mime)
            put(android.provider.MediaStore.MediaColumns.RELATIVE_PATH, android.os.Environment.DIRECTORY_PICTURES + "/PdfCraft")
        }
        val uri = resolver.insert(android.provider.MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) ?: error(context.getString(R.string.files_error_save_image))
        resolver.openOutputStream(uri).use { out ->
            requireNotNull(out) { context.getString(R.string.files_error_save_image) }
            file.inputStream().use { it.copyTo(out) }
        }
        return uri
    }

    /** Where [uri] is, as a person would say it: a folder path, or the app that provided it. */
    fun displayLocation(uri: Uri): String = when (uri.scheme) {
        ContentResolver.SCHEME_FILE -> uri.path?.substringBeforeLast('/')?.replace("/storage/emulated/0", context.getString(R.string.files_location_internal_storage)) ?: context.getString(R.string.files_location_this_device)
        else -> {
            val auth = uri.authority.orEmpty()
            when {
                auth.contains("downloads") -> context.getString(R.string.files_location_downloads)
                auth.contains("externalstorage") -> context.getString(R.string.files_location_internal_storage) + (Uri.decode(uri.lastPathSegment ?: "").substringAfter(':', "").substringBeforeLast('/', "").let { if (it.isEmpty()) "" else "/$it" })
                auth.contains("media") -> context.getString(R.string.files_location_media)
                else -> context.packageManager.resolveContentProvider(auth, 0)?.loadLabel(context.packageManager)?.toString() ?: context.getString(R.string.files_location_another_app)
            }
        }
    }

    companion object {
        /** A file name without path separators or control characters, ending in `.pdf`. */
        fun safeName(name: String): String {
            val clean = name.replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_").trim().trimStart('.').ifEmpty { "Document" }
            return if (clean.lowercase().endsWith(".pdf")) clean else "$clean.pdf"
        }

        private fun sha(s: String): String =
            MessageDigest.getInstance("SHA-256").digest(s.toByteArray()).take(12).joinToString("") { "%02x".format(it) }
    }
}
