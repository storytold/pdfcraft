package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** The tool the viewer's bottom bar has switched to (none: plain viewing). */
enum class ViewerTool {
    None,

    /** Comment: a note pinned where the page is tapped. */
    Comment,

    /** Highlight, underline or strikethrough over text dragged across ([TextMarkup]). */
    Markup,

    /** Freehand ink. */
    Draw,

    /** Free text typed onto the page. */
    Text,

    /** Fill & Sign: text, marks and signatures placed on a flat form. */
    FillSign,

    /** Edit PDF: the page's own text and images. */
    Edit,
}

/** The text-markup sub-tools, in the sub-toolbar's order. */
enum class TextMarkup { Highlight, Underline, Strikethrough }

/** The Comment tool's sub-tools: a typed note or a voice recording where the page is tapped. */
enum class CommentMode { Text, Voice }

/** What the bottom sheet over the viewer shows. */
enum class ViewerSheet { None, More, ViewSettings, Attachments, Bookmarks, Comments, MoreTools, Signatures }

/**
 * The viewer's own state for one document, beside the page view ([ViewerState]): whether the
 * bars show, the active tool, the open sheet, search, Night Mode, and when the page indicator last
 * had reason to show.
 */
@Stable
class ViewerController {
    /** The top bar and bottom toolbar (a tap on the page toggles them). */
    var barsVisible by mutableStateOf(true)

    var tool by mutableStateOf(ViewerTool.None)
    var markup by mutableStateOf(TextMarkup.Highlight)
    var commentMode by mutableStateOf(CommentMode.Text)

    var sheet by mutableStateOf(ViewerSheet.None)

    /** The active tool's open picker (colour, thickness, text size), and Fill & Sign's Sign panel. */
    var picker by mutableStateOf(Picker.None)
    var signPanel by mutableStateOf(false)

    /** Find is open in the top bar. */
    var searching by mutableStateOf(false)

    /** Pages drawn as inverted greyscale (View settings ▸ Night Mode). */
    var nightMode by mutableStateOf(false)

    /** Uptime (ms) of the last scroll or zoom: the scrubber and page pill show for a while after it. */
    var lastMoved by mutableLongStateOf(0L)

    /** The Pages screen (thumbnails, and organizing) covers the viewer. */
    var pagesOpen by mutableStateOf(false)

    /** The Go to page dialog. */
    var goToPage by mutableStateOf(false)

    /** Where a jump (link, bookmark, scrubber, Go to page) came from, for "back to page N". */
    var previousPage by mutableStateOf<Int?>(null)
    var previousAt by mutableLongStateOf(0L)

    /** Bars can't hide while a tool, search or sheet is up (as the reference's). */
    val barsLocked: Boolean get() = tool != ViewerTool.None || searching || sheet != ViewerSheet.None

    fun toggleBars() {
        if (!barsLocked) barsVisible = !barsVisible
    }

    /** Switch to [t]; tapping the active tool again goes back to plain viewing. */
    fun switchTool(t: ViewerTool) {
        tool = if (tool == t) ViewerTool.None else t
        picker = Picker.None
        signPanel = false
        barsVisible = true
        sheet = ViewerSheet.None
    }

    fun rememberJump(fromPage: Int) {
        previousPage = fromPage
        previousAt = android.os.SystemClock.uptimeMillis()
    }

    /** The "back to page N" link, while it is fresh (half an hour). */
    fun freshPreviousPage(now: Long): Int? = previousPage?.takeIf { now - previousAt < PREVIOUS_PAGE_MS }

    companion object {
        /** How long the scrubber and page pill stay after the last scroll or zoom. */
        const val INDICATOR_MS = 2000L
        const val PREVIOUS_PAGE_MS = 30L * 60 * 1000
    }
}
