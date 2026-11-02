package io.github.dikshant363.pdfcraft.ui

/**
 * A scan in progress as text, kept in the activity's saved state: Android often closes the app
 * while the camera app is in front, and the photo it brings back belongs to this (#38).
 *
 * [shot] is the file the camera is writing, [retake] the page it replaces, [forImage] when the
 * photo is for Add image rather than the scan.
 */
data class ScanSession(
    val shot: String?,
    val retake: Int?,
    val forImage: Boolean,
    val name: String,
    val pages: List<Page>,
) {
    class Page(val path: String, val corners: FloatArray, val found: FloatArray?)

    fun encode(): String = buildString {
        append(HEADER).append('\n')
        append("shot\t").append(shot.orEmpty()).append('\n')
        append("retake\t").append(retake?.toString().orEmpty()).append('\n')
        append("image\t").append(if (forImage) "1" else "0").append('\n')
        append("name\t").append(name.replace('\n', ' ').replace('\t', ' ')).append('\n')
        for (p in pages) {
            append("page\t").append(p.path).append('\t').append(p.corners.joinToString(","))
            append('\t').append(p.found?.joinToString(",").orEmpty()).append('\n')
        }
    }

    companion object {
        private const val HEADER = "pdfcraft-scan 1"

        /** The session in [text]; null when there is none or it isn't one. A malformed page is left out. */
        fun decode(text: String?): ScanSession? {
            val lines = text?.lines() ?: return null
            if (lines.firstOrNull() != HEADER) return null
            var shot: String? = null
            var retake: Int? = null
            var forImage = false
            var name = ""
            val pages = mutableListOf<Page>()
            for (line in lines.drop(1)) {
                val parts = line.split('\t')
                val value = parts.getOrNull(1).orEmpty()
                when (parts.first()) {
                    "shot" -> shot = value.ifEmpty { null }
                    "retake" -> retake = value.toIntOrNull()
                    "image" -> forImage = value == "1"
                    "name" -> name = value
                    "page" -> {
                        val corners = corners(parts.getOrNull(2)) ?: continue
                        if (value.isNotEmpty()) pages += Page(value, corners, corners(parts.getOrNull(3)))
                    }
                }
            }
            return ScanSession(shot, retake, forImage, name, pages)
        }

        // eight shares of the photo (x1, y1 … x4, y4), or null
        private fun corners(text: String?): FloatArray? {
            val values = text?.split(',')?.map { it.toFloatOrNull() ?: return null } ?: return null
            if (values.size != 8 || values.any { !it.isFinite() }) return null
            return values.toFloatArray()
        }
    }
}
