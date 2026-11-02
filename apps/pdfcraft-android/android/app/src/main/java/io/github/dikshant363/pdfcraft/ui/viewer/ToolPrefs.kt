package io.github.dikshant363.pdfcraft.ui.viewer

import android.content.Context
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import io.github.dikshant363.pdfcraft.R

/** The comment colours: 18 swatches in three rows of six (strong, light, greys), as measured on the reference. */
val CommentPalette: List<Pair<Color, Int>> = listOf(
    Color(0xFF0000FF) to R.string.prefs_color_blue, Color(0xFF6AD928) to R.string.prefs_color_green, Color(0xFFFFD100) to R.string.prefs_color_yellow,
    Color(0xFFFF7002) to R.string.prefs_color_orange, Color(0xFFE52237) to R.string.prefs_color_red, Color(0xFFA33086) to R.string.prefs_color_purple,
    Color(0xFF38E5FF) to R.string.prefs_color_light_blue, Color(0xFFC5FB72) to R.string.prefs_color_light_green, Color(0xFFFCF485) to R.string.prefs_color_light_yellow,
    Color(0xFFFFC09E) to R.string.prefs_color_light_orange, Color(0xFFFF809D) to R.string.prefs_color_pink, Color(0xFFFB88FF) to R.string.prefs_color_light_purple,
    Color(0xFFFFFFFF) to R.string.prefs_color_white, Color(0xFFCCCCCC) to R.string.prefs_color_pale_grey, Color(0xFFAAAAAA) to R.string.prefs_color_light_grey,
    Color(0xFF777777) to R.string.prefs_color_medium_grey, Color(0xFF444444) to R.string.prefs_color_dark_grey, Color(0xFF000000) to R.string.prefs_color_black,
)

/** The seven ink widths of the stroke picker, in points. */
val InkWidths = listOf(1f, 2f, 3f, 4f, 6f, 8f, 10f)

/** Free-text sizes: 6 to 22 pt in steps of 2. */
val TextSizes = (6..22 step 2).map { it.toFloat() }

/** A tool that has its own colour and opacity. */
enum class StyledTool(val key: String, val defaultColor: Long) {
    Highlight("highlight", 0xFFFFD100),
    Underline("underline", 0xFFC5FB72),
    Strikethrough("strikethrough", 0xFFE52237),
    Ink("ink", 0xFFE52237),
    FreeText("freetext", 0xFF0000FF),
    Note("note", 0xFFFFD800),
}

/**
 * What each comment tool last used (colour, opacity, ink width, text size), kept in the app's
 * preferences as the reference keeps them per tool.
 */
@Stable
class ToolPrefs(context: Context) {
    private val prefs = context.getSharedPreferences("tools", Context.MODE_PRIVATE)

    private val colors = StyledTool.entries.associateWith { mutableIntStateOf(prefs.getInt("${it.key}.color", it.defaultColor.toInt())) }
    private val opacities = StyledTool.entries.associateWith { mutableFloatStateOf(prefs.getFloat("${it.key}.opacity", 1f)) }

    var inkWidth by mutableFloatStateOf(prefs.getFloat("ink.width", 1f))
        private set
    var textSize by mutableFloatStateOf(prefs.getFloat("freetext.size", 16f))
        private set

    /** Fill & Sign's one colour for everything it places, and its text size. */
    var fillColor by mutableIntStateOf(prefs.getInt("fill.color", 0xFF000000.toInt()))
        private set
    var fillTextSize by mutableFloatStateOf(prefs.getFloat("fill.size", 12f))
        private set

    fun color(t: StyledTool): Color = Color(colors.getValue(t).intValue)
    fun opacity(t: StyledTool): Float = opacities.getValue(t).floatValue

    fun setColor(t: StyledTool, c: Color) {
        val argb = c.toArgbInt()
        colors.getValue(t).intValue = argb
        prefs.edit().putInt("${t.key}.color", argb).apply()
    }

    fun setOpacity(t: StyledTool, o: Float) {
        val v = o.coerceIn(0.25f, 1f)
        opacities.getValue(t).floatValue = v
        prefs.edit().putFloat("${t.key}.opacity", v).apply()
    }

    /** Draw ▸ Snap to shape: strokes that look like a line, box, oval or polygon become one. */
    var snapToShape by mutableStateOf(prefs.getBoolean("ink.snap", true))
        private set

    fun updateSnapToShape(on: Boolean) {
        snapToShape = on
        prefs.edit().putBoolean("ink.snap", on).apply()
    }

    /** Shows the "Snap to shape on" hint on entering Draw, the first few times only. */
    /** Whether a tool's start hint is still shown: the first four times for each tool, as the reference does. */
    fun takeToolHint(key: String): Boolean {
        val n = prefs.getInt("hint.$key", 0)
        if (n >= 4) return false
        prefs.edit().putInt("hint.$key", n + 1).apply()
        return true
    }

    fun takeSnapHint(): Boolean {
        val n = prefs.getInt("ink.snapHints", 0)
        if (n >= 3) return false
        prefs.edit().putInt("ink.snapHints", n + 1).apply()
        return true
    }

    fun updateInkWidth(w: Float) {
        inkWidth = w
        prefs.edit().putFloat("ink.width", w).apply()
    }

    fun updateTextSize(s: Float) {
        textSize = s
        prefs.edit().putFloat("freetext.size", s).apply()
    }

    fun setFillColor(c: Color) {
        fillColor = c.toArgbInt()
        prefs.edit().putInt("fill.color", fillColor).apply()
    }

    fun updateFillTextSize(s: Float) {
        fillTextSize = s.coerceIn(3f, 100f)
        prefs.edit().putFloat("fill.size", fillTextSize).apply()
    }

    /** The author name put on comments (Preferences ▸ Author name). */
    var author: String
        get() = prefs.getString("author", "") ?: ""
        set(v) = prefs.edit().putString("author", v).apply()

    /** Whether the reader has been asked for their name once (the first time they comment). */
    var authorAsked: Boolean
        get() = prefs.getBoolean("authorAsked", false)
        set(v) = prefs.edit().putBoolean("authorAsked", v).apply()
}

/** `#RRGGBB` for the engine's tools. */
fun Color.hex(): String = "#%06X".format(toArgbInt() and 0xFFFFFF)

fun Color.toArgbInt(): Int {
    val a = (alpha * 255f + 0.5f).toInt() and 0xFF
    val r = (red * 255f + 0.5f).toInt() and 0xFF
    val g = (green * 255f + 0.5f).toInt() and 0xFF
    val b = (blue * 255f + 0.5f).toInt() and 0xFF
    return (a shl 24) or (r shl 16) or (g shl 8) or b
}

val TextMarkup.styled: StyledTool
    get() = when (this) {
        TextMarkup.Highlight -> StyledTool.Highlight
        TextMarkup.Underline -> StyledTool.Underline
        TextMarkup.Strikethrough -> StyledTool.Strikethrough
    }

/** The engine's comment type for a markup tool. */
val TextMarkup.engineType: String
    get() = when (this) {
        TextMarkup.Highlight -> "highlight"
        TextMarkup.Underline -> "underline"
        TextMarkup.Strikethrough -> "strikeout"
    }
