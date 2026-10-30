package com.storytold.pdfcraft

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest

/**
 * High-performance two-tier cache (memory LruCache + disk cache) for PDF document page thumbnails.
 */
object RecentThumbnailManager {
    private val memoryCache = object : LruCache<String, Bitmap>(32) {
        override fun sizeOf(key: String, bitmap: Bitmap): Int {
            return (bitmap.byteCount / 1024).coerceAtLeast(1)
        }
    }

    private fun hashKey(uriString: String): String {
        return try {
            val md = MessageDigest.getInstance("MD5")
            val digest = md.digest(uriString.toByteArray(Charsets.UTF_8))
            digest.joinToString("") { "%02x".format(it) }
        } catch (_: Exception) {
            uriString.hashCode().toString()
        }
    }

    private fun getThumbFile(context: Context, uriString: String): File {
        val dir = File(context.cacheDir, "pdf_thumbnails").apply {
            if (!exists()) mkdirs()
        }
        return File(dir, "${hashKey(uriString)}.png")
    }

    fun getThumbnail(context: Context, uriString: String): Bitmap? {
        val key = hashKey(uriString)
        memoryCache.get(key)?.let { return it }

        val file = getThumbFile(context, uriString)
        if (file.exists() && file.length() > 0) {
            try {
                val bitmap = BitmapFactory.decodeFile(file.absolutePath)
                if (bitmap != null) {
                    memoryCache.put(key, bitmap)
                    return bitmap
                }
            } catch (_: Exception) {}
        }
        return null
    }

    fun saveThumbnail(context: Context, uriString: String, bitmap: Bitmap) {
        val key = hashKey(uriString)
        memoryCache.put(key, bitmap)
        try {
            val file = getThumbFile(context, uriString)
            FileOutputStream(file).use { out ->
                bitmap.compress(Bitmap.CompressFormat.PNG, 90, out)
            }
        } catch (_: Exception) {}
    }

    fun removeThumbnail(context: Context, uriString: String) {
        val key = hashKey(uriString)
        memoryCache.remove(key)
        try {
            val file = getThumbFile(context, uriString)
            if (file.exists()) {
                file.delete()
            }
        } catch (_: Exception) {}
    }
}
