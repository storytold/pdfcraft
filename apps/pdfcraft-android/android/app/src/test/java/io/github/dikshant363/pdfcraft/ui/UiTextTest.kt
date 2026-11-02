package io.github.dikshant363.pdfcraft.ui

import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The text people read comes from res/values/strings.xml, so a translation covers all of it:
 * English written straight into a Text, a label or a message would stay English in every language.
 */
class UiTextTest {
    // a call or argument that shows its string, followed by a string with words in it
    private val shown = Regex(
        """\b(?:Text|tell|SheetRow|SheetTitle|BarIcon|ToolTile|QuickItem|OrganizeItem)\(\s*"([^"]*)"""" +
            """|\b(?:contentDescription|title|label|placeholder|confirm|dismiss|actionLabel|text)\s*=\s*"([^"]*)"""",
    )

    // file types and the letters drawn as icons are the same in every language
    private val sameEverywhere = setOf("PDF", "Ag")

    @Test
    fun noEnglishInTheCode() {
        val sources = File("src/main/java").walk().filter { it.extension == "kt" }.toList()
        assertTrue("no sources found", sources.isNotEmpty())
        val found = sources.flatMap { f ->
            f.readLines().withIndex().flatMap { (i, line) ->
                val comment = line.trimStart().let { it.startsWith("*") || it.startsWith("/*") }
                val code = if (comment) "" else line.substringBefore("// ")
                shown.findAll(code).mapNotNull { m ->
                    val s = m.groupValues[1].ifEmpty { m.groupValues[2] }
                    // words a person reads: a capital and lower case, or a space between letters
                    val words = Regex("""^[A-Z][a-z]|[A-Za-z] [A-Za-z]""").containsMatchIn(s)
                    if (words && s !in sameEverywhere) "${f.name}:${i + 1}: \"$s\"" else null
                }.toList()
            }
        }
        assertTrue("Text written in the code instead of res/values/strings.xml:\n" + found.joinToString("\n"), found.isEmpty())
    }
}
