package io.github.dikshant363.pdfcraft.engine

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.content.ContentUris
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.fileThumbnail
import java.io.File
import java.security.MessageDigest

/** A PDF on the phone: where it is, its name, folder, size and when it was last changed. */
data class LocalPdf(val uri: Uri, val path: String?, val name: String, val folder: String, val size: Long, val modified: Long)

/**
 * The PDFs in the folders the user has chosen for "On this device", and the ones PdfCraft saved
 * itself (copies, scans, combined files in Downloads, which the media store shows an app without
 * any permission), newest first. Folders are picked with the system's folder picker (no access to
 * all files): the app keeps a lasting grant for each one and reads, saves, renames and deletes its
 * files through the document provider.
 */
object LocalFiles {
    /** At most this many entries are looked at in all, and folders this deep, so a huge tree can't stall the list. */
    private const val MAX_ENTRIES = 20_000
    private const val MAX_DEPTH = 8

    /** The folders the user has added (the lasting grants that are folders). */
    fun folders(context: Context): List<Uri> =
        runCatching { context.contentResolver.persistedUriPermissions }.getOrDefault(emptyList())
            .filter { it.isReadPermission && runCatching { DocumentsContract.isTreeUri(it.uri) }.getOrDefault(false) }
            .map { it.uri }

    /** Whether any folder has been added. */
    fun hasAccess(context: Context): Boolean = folders(context).isNotEmpty()

    /** Keep [tree] (from the folder picker) for good: with writing when the provider allows it. */
    fun addFolder(context: Context, tree: Uri) {
        val resolver = context.contentResolver
        runCatching { resolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION) }
            .recoverCatching { resolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
    }

    /** Stop listing [tree] and give its grant back. */
    fun removeFolder(context: Context, tree: Uri) {
        val resolver = context.contentResolver
        runCatching { resolver.releasePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION) }
            .recoverCatching { resolver.releasePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
    }

    /** A folder's name as its provider gives it ("Download", "Documents"). */
    fun folderName(context: Context, tree: Uri): String = runCatching {
        val doc = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        context.contentResolver.query(doc, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    }.getOrNull() ?: tree.lastPathSegment?.substringAfterLast(':')?.ifEmpty { null } ?: context.getString(R.string.files_folder_unnamed)

    fun scan(context: Context): List<LocalPdf> {
        val resolver = context.contentResolver
        val out = ArrayList<LocalPdf>()
        val seen = HashSet<String>()
        var looked = 0
        val cols = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        )
        for (tree in folders(context)) {
            val rootId = runCatching { DocumentsContract.getTreeDocumentId(tree) }.getOrNull() ?: continue
            // breadth first: (folder id, its name, depth)
            val queue = ArrayDeque<Triple<String, String, Int>>()
            queue.add(Triple(rootId, folderName(context, tree), 0))
            while (queue.isNotEmpty() && looked < MAX_ENTRIES) {
                val (dirId, dirName, depth) = queue.removeFirst()
                val children = runCatching { DocumentsContract.buildChildDocumentsUriUsingTree(tree, dirId) }.getOrNull() ?: continue
                runCatching {
                    resolver.query(children, cols, null, null, null)?.use { c ->
                        while (c.moveToNext() && looked < MAX_ENTRIES) {
                            looked++
                            val id = c.getString(0) ?: continue
                            val name = c.getString(1) ?: continue
                            if (name.startsWith(".")) continue
                            val mime = c.getString(2)
                            if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                                if (depth + 1 <= MAX_DEPTH) queue.add(Triple(id, name, depth + 1))
                                continue
                            }
                            if (mime != "application/pdf" && !name.endsWith(".pdf", ignoreCase = true)) continue
                            // a folder added twice (or inside another) lists its files once
                            if (!seen.add(id)) continue
                            val uri = DocumentsContract.buildDocumentUriUsingTree(tree, id)
                            val size = if (c.isNull(3)) 0L else c.getLong(3)
                            val modified = if (c.isNull(4)) 0L else c.getLong(4)
                            out.add(LocalPdf(uri, null, name, dirName, size, modified))
                        }
                    }
                }
            }
        }
        out.addAll(ownFiles(context))
        out.sortByDescending { it.modified }
        return out
    }

    /** The PDFs this app put in shared storage itself (the media store lists only those to it). */
    private fun ownFiles(context: Context): List<LocalPdf> {
        val out = ArrayList<LocalPdf>()
        val collection = MediaStore.Files.getContentUri("external")
        val projection = arrayOf(
            MediaStore.Files.FileColumns._ID,
            MediaStore.Files.FileColumns.DISPLAY_NAME,
            MediaStore.Files.FileColumns.SIZE,
            MediaStore.Files.FileColumns.DATE_MODIFIED,
            MediaStore.Files.FileColumns.RELATIVE_PATH,
        )
        val selection = "${MediaStore.Files.FileColumns.MIME_TYPE} = ? OR ${MediaStore.Files.FileColumns.DISPLAY_NAME} LIKE ?"
        runCatching {
            context.contentResolver.query(collection, projection, selection, arrayOf("application/pdf", "%.pdf"), null)?.use { c ->
                while (c.moveToNext()) {
                    val name = c.getString(1) ?: continue
                    if (name.startsWith(".")) continue
                    val folder = c.getString(4)?.trimEnd('/')?.substringAfterLast('/') ?: ""
                    out.add(LocalPdf(ContentUris.withAppendedId(collection, c.getLong(0)), null, name, folder, c.getLong(2), c.getLong(3) * 1000))
                }
            }
        }
        return out
    }
}

/**
 * Small pictures of documents' first pages for file lists, rendered by the engine and kept on
 * disk (keyed by the file and its last change), so a list scrolls without re-rendering. A file is
 * given by its path, or by its content address (a document in a chosen folder), which is copied
 * to a scratch file just long enough to draw it.
 */
class Thumbnails(private val context: Context) {
    private val dir = File(context.cacheDir, "thumbs").apply { mkdirs() }
    private val memory = object : android.util.LruCache<String, Bitmap>(12 * 1024 * 1024) {
        override fun sizeOf(key: String, value: Bitmap) = value.byteCount
    }

    /** The thumbnail of the PDF at [path] or content address (null when it can't be drawn: protected, damaged). Blocking. */
    fun of(path: String, maxSide: Int = 160): Bitmap? {
        if (path.startsWith("content:")) return ofContent(Uri.parse(path), maxSide)
        val f = File(path)
        if (!f.isFile) return null
        return cached(sha("$path|${f.lastModified()}|${f.length()}|$maxSide")) { fileThumbnail(path, maxSide.toUInt()).toBitmap() }
    }

    private fun ofContent(uri: Uri, maxSide: Int): Bitmap? {
        val resolver = context.contentResolver
        val (size, modified) = runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.SIZE, DocumentsContract.Document.COLUMN_LAST_MODIFIED), null, null, null)?.use { c ->
                if (!c.moveToFirst()) null
                else (if (c.isNull(0)) null else c.getLong(0)) to (if (c.columnCount > 1 && !c.isNull(1)) c.getLong(1) else 0L)
            }
        }.getOrNull() ?: return null
        // a very large file or one whose size is unknown isn't copied just for its first page
        if (size == null || size <= 0L || size > MAX_COPY) return null
        return cached(sha("$uri|$modified|$size|$maxSide")) {
            val tmp = File.createTempFile("thumb", ".pdf", context.cacheDir)
            try {
                resolver.openInputStream(uri)?.use { input -> tmp.outputStream().use { input.copyTo(it) } } ?: error("unreadable")
                fileThumbnail(tmp.absolutePath, maxSide.toUInt()).toBitmap()
            } finally {
                tmp.delete()
            }
        }
    }

    private fun cached(key: String, draw: () -> Bitmap): Bitmap? {
        memory.get(key)?.let { return it }
        val file = File(dir, "$key.png")
        if (file.isFile) {
            BitmapFactory.decodeFile(file.absolutePath)?.let {
                memory.put(key, it)
                return it
            }
        }
        val bmp = runCatching(draw).getOrNull() ?: return null
        runCatching { file.outputStream().use { bmp.compress(Bitmap.CompressFormat.PNG, 90, it) } }
        memory.put(key, bmp)
        return bmp
    }

    private fun sha(s: String) = MessageDigest.getInstance("SHA-1").digest(s.toByteArray()).joinToString("") { "%02x".format(it) }

    private companion object {
        const val MAX_COPY = 100L * 1024 * 1024
    }
}
