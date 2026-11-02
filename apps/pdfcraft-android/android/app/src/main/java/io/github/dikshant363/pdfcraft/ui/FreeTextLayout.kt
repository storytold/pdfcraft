package io.github.dikshant363.pdfcraft.ui

/**
 * How typed text sits in a new text box, measured the way the engine draws the box (its Helvetica
 * width table, 2 pt inside the edge, lines 1.2 × the size apart), so the box the app asks for holds
 * exactly the lines the engine draws: nothing wraps again, nothing is cut off.
 */
internal object FreeTextLayout {
    /** Room between the box's edge and its text (the engine's, for a box with no border). */
    const val PAD = 2f

    /** The width of [s] at [size] points, as the engine measures Helvetica. */
    fun width(s: String, size: Float): Float = s.sumOf { c ->
        when (c) {
            ' ', 'i', 'j', 'l', '\'', '!', '|', '.', ',', ':', ';', 'I' -> 260
            'f', 't', 'r', '(', ')', '[', ']', '/', '-', '"' -> 333
            'm', 'M' -> 833
            'w' -> 722
            'W' -> 944
            'J', 'c', 'k', 's', 'v', 'x', 'y', 'z' -> 500
            in '0'..'9', in 'a'..'z', '$', '#', '?', '_' -> 556
            in 'A'..'Z' -> 680
            '@' -> 1015
            else -> if (c.isWhitespace()) 260 else 584
        }.toInt()
    } * size / 1000f

    /** [text] broken into lines no wider than [maxWidth] (at spaces; a word too long, by letters). */
    fun wrap(text: String, size: Float, maxWidth: Float): List<String> {
        val lines = mutableListOf<String>()
        for (para in text.split('\n', '\r')) {
            var line = ""
            for (word in para.split(' ')) {
                val candidate = if (line.isEmpty()) word else "$line $word"
                if (width(candidate, size) <= maxWidth || line.isEmpty() && width(word, size) <= maxWidth) {
                    line = candidate
                    continue
                }
                if (line.isNotEmpty()) lines += line.also { line = "" }
                for (ch in word) {
                    if (line.isNotEmpty() && width(line + ch, size) > maxWidth) lines += line.also { line = "" }
                    line += ch
                }
            }
            lines += line
        }
        return lines
    }

    /**
     * The lines of [text] when the box may grow to [room] points wide, and the box's width and height:
     * as wide as the longest line, as tall as the lines (the engine's first baseline is 0.9 × the size
     * below the padding, and the descenders need about 0.3 × the size under the last one).
     */
    fun layout(text: String, size: Float, room: Float): Triple<List<String>, Float, Float> {
        val lines = wrap(text, size, (room - 2 * PAD).coerceAtLeast(size))
        val longest = lines.maxOfOrNull { width(it, size) } ?: 0f
        // a point to spare, so a line the full width doesn't break again on rounding
        val w = (longest + 2 * PAD + 1f).coerceAtLeast(size * 2)
        val h = 2 * PAD + size * 0.9f + (lines.size - 1).coerceAtLeast(0) * size * 1.2f + size * 0.3f
        return Triple(lines, w, h)
    }
}
