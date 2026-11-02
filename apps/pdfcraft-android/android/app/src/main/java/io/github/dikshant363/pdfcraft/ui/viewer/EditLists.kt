package io.github.dikshant363.pdfcraft.ui.viewer

import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.text as appText

/**
 * Bulleted and numbered lists in Edit PDF: each line of a paragraph (its hard line breaks) becomes
 * an item with a marker in front. The markers are ones the standard PDF fonts carry (a bullet, an
 * en dash, numbers, letters and small roman numerals), so any paragraph can take them.
 */
enum class ListStyle(private val sample: String?) {
    None(null),
    Bullet("•"),
    Dash("–"),
    Number("1."),
    Lower("a."),
    Upper("A."),
    Roman("i.");

    /** What the list panel shows for the style: its first marker, or the word for no list. */
    val label: String get() = sample ?: appText(R.string.lists_none)

    /** The marker of item [n] (0-based). */
    fun marker(n: Int): String = when (this) {
        None -> ""
        Bullet -> "•"
        Dash -> "–"
        Number -> "${n + 1}."
        Lower -> "${letters(n, 'a')}."
        Upper -> "${letters(n, 'A')}."
        Roman -> "${roman(n + 1)}."
    }

    companion object {
        private val patterns = mapOf(
            Bullet to Regex("^•\\s+"),
            Dash to Regex("^–\\s+"),
            Number to Regex("^\\d{1,3}\\.\\s+"),
            Roman to Regex("^[ivxlcdm]{1,7}\\.\\s+"),
            Lower to Regex("^[a-z]{1,2}\\.\\s+"),
            Upper to Regex("^[A-Z]{1,2}\\.\\s+"),
        )

        /** The list style every line of [text] already has, or [None]. */
        fun of(text: String): ListStyle {
            val lines = text.split('\n').filter { it.isNotBlank() }
            if (lines.isEmpty()) return None
            // roman before letters: "i." is both, and a list of i., ii., iii. is roman
            for (style in listOf(Bullet, Dash, Number, Roman, Lower, Upper)) {
                val p = patterns.getValue(style)
                if (lines.all { p.containsMatchIn(it) } && lines.withIndex().all { (i, l) -> l.startsWith(style.marker(i) + " ") || style == Bullet || style == Dash }) return style
            }
            return None
        }

        /** [text] as a list of [style] (or a plain paragraph for [None]), replacing any list it was. */
        fun apply(text: String, style: ListStyle): String {
            val current = of(text)
            val lines = text.split('\n')
            val plain = if (current == None) lines else lines.map { l -> patterns.getValue(current).replaceFirst(l, "") }
            if (style == None) return plain.joinToString("\n")
            var n = 0
            return plain.joinToString("\n") { l -> if (l.isBlank()) l else "${style.marker(n++)} $l" }
        }

        private fun letters(n: Int, a: Char): String = if (n < 26) "${a + n}" else "${a + (n / 26 - 1)}${a + n % 26}"

        private fun roman(v: Int): String {
            val parts = listOf(1000 to "m", 900 to "cm", 500 to "d", 400 to "cd", 100 to "c", 90 to "xc", 50 to "l", 40 to "xl", 10 to "x", 9 to "ix", 5 to "v", 4 to "iv", 1 to "i")
            var rest = v
            val sb = StringBuilder()
            for ((k, s) in parts) while (rest >= k) { sb.append(s); rest -= k }
            return sb.toString()
        }
    }
}
