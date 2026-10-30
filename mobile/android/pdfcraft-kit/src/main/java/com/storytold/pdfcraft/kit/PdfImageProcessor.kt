package com.storytold.pdfcraft.kit

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import java.io.ByteArrayOutputStream

/**
 * Robust image processor for mobile camera captures and photo imports.
 *
 * Guarantees that images (including HEIC, WebP, AVIF, PNG, JPEG) are:
 * 1. Read safely without OutOfMemory errors using subsampling.
 * 2. Rotated upright according to their EXIF orientation tag.
 * 3. Scaled within safe bounds (default max 2560px on the longest dimension).
 * 4. Encoded to clean, standard JPEG bytes compatible with the engine.
 */
object PdfImageProcessor {

    fun processImageUriToJpegBytes(
        context: Context,
        uri: Uri,
        maxDimension: Int = 2560
    ): ByteArray? {
        return try {
            // 1. Read EXIF orientation
            var orientation = ExifInterface.ORIENTATION_NORMAL
            try {
                context.contentResolver.openInputStream(uri)?.use { stream ->
                    val exif = ExifInterface(stream)
                    orientation = exif.getAttributeInt(
                        ExifInterface.TAG_ORIENTATION,
                        ExifInterface.ORIENTATION_NORMAL
                    )
                }
            } catch (_: Throwable) {
                // Ignore EXIF read errors
            }

            // 2. Decode bounds to compute subsampling
            val boundsOptions = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            context.contentResolver.openInputStream(uri)?.use { stream ->
                BitmapFactory.decodeStream(stream, null, boundsOptions)
            }

            val origWidth = boundsOptions.outWidth
            val origHeight = boundsOptions.outHeight
            if (origWidth <= 0 || origHeight <= 0) {
                return null
            }

            var sampleSize = 1
            var w = origWidth
            var h = origHeight
            while (w / 2 >= maxDimension || h / 2 >= maxDimension) {
                w /= 2
                h /= 2
                sampleSize *= 2
            }

            // 3. Decode scaled bitmap
            val decodeOptions = BitmapFactory.Options().apply {
                inSampleSize = sampleSize
                inPreferredConfig = Bitmap.Config.ARGB_8888
            }

            val decodedBitmap = context.contentResolver.openInputStream(uri)?.use { stream ->
                BitmapFactory.decodeStream(stream, null, decodeOptions)
            } ?: return null

            // 4. Transform orientation and final scale
            val matrix = Matrix()
            when (orientation) {
                ExifInterface.ORIENTATION_ROTATE_90 -> matrix.postRotate(90f)
                ExifInterface.ORIENTATION_ROTATE_180 -> matrix.postRotate(180f)
                ExifInterface.ORIENTATION_ROTATE_270 -> matrix.postRotate(270f)
                ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> matrix.postScale(-1f, 1f)
                ExifInterface.ORIENTATION_FLIP_VERTICAL -> matrix.postScale(1f, -1f)
            }

            val curWidth = decodedBitmap.width
            val curHeight = decodedBitmap.height
            val maxSide = maxOf(curWidth, curHeight)
            if (maxSide > maxDimension) {
                val scale = maxDimension.toFloat() / maxSide.toFloat()
                matrix.postScale(scale, scale)
            }

            val finalBitmap = if (!matrix.isIdentity) {
                val transformed = Bitmap.createBitmap(
                    decodedBitmap,
                    0, 0,
                    curWidth, curHeight,
                    matrix,
                    true
                )
                if (transformed != decodedBitmap) {
                    decodedBitmap.recycle()
                }
                transformed
            } else {
                decodedBitmap
            }

            // 5. Compress to standard JPEG bytes
            val out = ByteArrayOutputStream()
            finalBitmap.compress(Bitmap.CompressFormat.JPEG, 90, out)
            finalBitmap.recycle()
            out.toByteArray()
        } catch (e: Throwable) {
            e.printStackTrace()
            null
        }
    }
}
