package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.CommentInfo
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlin.math.roundToInt

/** A note waiting for its text: at a point (Comment tool) or on selected text (Add comment). */
class NotePrompt(val doc: OpenDocument, val at: PagePoint?, val selection: Selection? = null)

/** Free text waiting to be typed at a point (Text tool). */
class TextPrompt(val doc: OpenDocument, val at: PagePoint)

/** A reply waiting to be written. */
class ReplyPrompt(val doc: OpenDocument, val comment: CommentInfo)

/**
 * The scrubber on the right edge: a pill with the page number that follows the scroll position
 * and can be dragged to move through the document; a tap opens Go to page.
 */
@Composable
fun BoxScope.Scrubber(doc: OpenDocument, visible: Boolean, top: Dp, bottom: Dp, onDrag: () -> Unit, onTap: () -> Unit) {
    val c = pdfColors
    val v = doc.viewer
    val density = LocalDensity.current
    var dragging by remember { mutableStateOf(false) }
    AnimatedVisibility(visible || dragging, modifier = Modifier.align(Alignment.TopEnd), enter = fadeIn(tween(150)), exit = fadeOut(tween(300))) {
        BoxWithConstraints(Modifier.fillMaxSize().padding(top = top + 8.dp, bottom = bottom + 8.dp)) {
            val track = with(density) { maxHeight.toPx() }
            val pillH = with(density) { 32.dp.toPx() }
            val maxScroll = (v.layout.height * v.zoom - v.viewH).coerceAtLeast(1f)
            val fraction = (v.scrollY / maxScroll).coerceIn(0f, 1f)
            val y = fraction * (track - pillH)
            val label = "${v.currentPage + 1}"
            val pageOf = stringResource(R.string.viewer_page_of, v.currentPage + 1, doc.pageCount)
            Box(
                Modifier
                    .align(Alignment.TopEnd)
                    .offset { IntOffset(0, y.roundToInt()) }
                    .height(32.dp)
                    .widthIn(min = if (label.length >= 3) 52.dp else 40.dp)
                    .shadow(2.dp, RoundedCornerShape(topStart = 16.dp, bottomStart = 16.dp))
                    .background(c.surface, RoundedCornerShape(topStart = 16.dp, bottomStart = 16.dp))
                    .pointerInput(doc) {
                        detectTapGestures(onTap = { onTap() })
                    }
                    .pointerInput(doc) {
                        detectDragGestures(
                            onDragStart = { dragging = true },
                            onDragEnd = { dragging = false },
                            onDragCancel = { dragging = false },
                        ) { change, drag ->
                            change.consume()
                            val newFraction = ((v.scrollY / maxScroll) + drag.y / (track - pillH).coerceAtLeast(1f)).coerceIn(0f, 1f)
                            v.setScroll(v.scrollX, newFraction * maxScroll)
                            onDrag()
                        }
                    }
                    .semantics { contentDescription = pageOf },
                contentAlignment = Alignment.Center,
            ) {
                Text(label, color = c.text, fontSize = 15.sp, modifier = Modifier.padding(start = 12.dp, end = 10.dp))
            }
        }
    }
}

/** Go to page: a page number from 1 to N, and the way back to where the last jump came from. */
@Composable
fun GoToPageDialog(doc: OpenDocument, onDismiss: () -> Unit) {
    val c = pdfColors
    var text by remember { mutableStateOf("") }
    var wrong by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val previous = doc.controller.freshPreviousPage(android.os.SystemClock.uptimeMillis())
    fun go() {
        val n = text.trim().toIntOrNull()
        if (n == null || n < 1 || n > doc.pageCount) {
            wrong = true
            text = ""
            return
        }
        doc.controller.rememberJump(doc.viewer.currentPage)
        doc.viewer.goToPage(n - 1)
        onDismiss()
    }
    io.github.dikshant363.pdfcraft.ui.PdfDialog(onDismiss = onDismiss, title = stringResource(R.string.viewer_go_to_page), confirm = stringResource(R.string.common_ok), onConfirm = ::go, confirmEnabled = text.isNotEmpty()) {
            Column {
                Text(stringResource(R.string.viewer_go_to_page_range, doc.pageCount), color = c.textSecondary, fontSize = 14.sp)
                Spacer(Modifier.height(10.dp))
                io.github.dikshant363.pdfcraft.ui.PdfField(
                    text, { text = it.filter(Char::isDigit).take(6); wrong = false }, stringResource(R.string.viewer_enter_page_number), error = wrong, focus = focus,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = androidx.compose.ui.text.input.ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { go() }),
                )
                if (previous != null) {
                    Spacer(Modifier.height(12.dp))
                    Text(
                        stringResource(R.string.viewer_back_to_page, previous + 1),
                        color = c.accent,
                        modifier = Modifier.clickable {
                            doc.viewer.goToPage(previous)
                            doc.controller.previousPage = null
                            onDismiss()
                        }.padding(vertical = 4.dp),
                    )
                }
            }
    }
}

/**
 * The menu by selected text: Add comment, Highlight, Underline and Strikethrough as icons, then
 * Edit text, Copy text and Read aloud. Like the reference's it sits below the selection when there
 * is room, else above.
 */
@Composable
fun BoxScope.SelectionMenu(vm: AppViewModel, doc: OpenDocument, sel: Selection) {
    val c = pdfColors
    val density = LocalDensity.current
    val page = sel.range.page.toInt()
    val first = sel.rects.firstOrNull() ?: return
    val last = sel.rects.lastOrNull() ?: return
    val top = doc.viewer.pageToScreen(page, first[0], first[1])
    val bottom = doc.viewer.pageToScreen(page, last[2], last[3])
    var menuSize by remember { mutableStateOf(androidx.compose.ui.unit.IntSize.Zero) }
    val margin = with(density) { 29.dp.toPx() }
    val maxBottom = doc.viewer.viewH - doc.viewer.padBottom
    val below = bottom.y + margin
    val y = if (below + menuSize.height < maxBottom) below else (top.y - margin - menuSize.height).coerceAtLeast(doc.viewer.padTop)
    val x = ((top.x + bottom.x) / 2f - menuSize.width / 2f).coerceIn(with(density) { 8.dp.toPx() }, (doc.viewer.viewW - menuSize.width - with(density) { 8.dp.toPx() }).coerceAtLeast(0f))
    val allowed = doc.info.allowsAnnotation
    val prefs = vm.toolPrefs
    fun markup(m: TextMarkup) {
        vm.addMarkup(doc, m, page, sel.rects, prefs.color(m.styled), prefs.opacity(m.styled))
        doc.selection.clear()
    }
    Column(
        Modifier
            .offset { IntOffset(x.roundToInt(), y.roundToInt()) }
            .onSizeChanged { menuSize = it }
            .width(209.dp)
            .shadow(6.dp, RoundedCornerShape(4.dp))
            .background(c.menu, RoundedCornerShape(4.dp)),
    ) {
        Row(Modifier.fillMaxWidth().height(61.dp), horizontalArrangement = Arrangement.SpaceEvenly, verticalAlignment = Alignment.CenterVertically) {
            MenuIcon(LucideIcons.MessageSquarePlus, stringResource(R.string.viewer_add_comment), allowed) { vm.notePrompt = NotePrompt(doc, null, sel) }
            MenuIcon(LucideIcons.Highlighter, stringResource(R.string.common_tool_highlight), allowed) { markup(TextMarkup.Highlight) }
            MenuIcon(LucideIcons.Underline, stringResource(R.string.common_tool_underline), allowed) { markup(TextMarkup.Underline) }
            MenuIcon(LucideIcons.Strikethrough, stringResource(R.string.viewer_strikethrough), allowed) { markup(TextMarkup.Strikethrough) }
        }
        // an inset divider, then the rows with 8 dp above and below, as the reference's menu
        Box(Modifier.fillMaxWidth().padding(horizontal = 4.dp).height(1.dp).background(c.divider))
        Spacer(Modifier.height(8.dp))
        if (doc.writable || doc.source == null) MenuRow(LucideIcons.FilePenLine, stringResource(R.string.viewer_edit_text)) {
            doc.selection.clear()
            vm.enterTool(doc, ViewerTool.Edit)
        }
        if (doc.info.allowsCopying) MenuRow(LucideIcons.Copy, stringResource(R.string.viewer_copy_text)) { vm.copySelection(doc) }
        // while reading aloud, the reading moves to the selection (the reference's "from here")
        MenuRow(LucideIcons.Volume2, stringResource(if (vm.readAloud != null) R.string.viewer_read_from_here else R.string.common_tool_read_aloud)) {
            vm.startReadAloud(doc, fromSelection = true)
            doc.selection.clear()
        }
        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun MenuIcon(icon: ImageVector, label: String, enabled: Boolean, onClick: () -> Unit) {
    val c = pdfColors
    Box(
        Modifier.size(48.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick).semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, tint = if (enabled) c.icon else c.icon.copy(alpha = 0.35f), modifier = Modifier.size(24.dp))
    }
}

@Composable
private fun MenuRow(icon: ImageVector, label: String, onClick: () -> Unit) {
    val c = pdfColors
    Row(Modifier.fillMaxWidth().height(41.dp).clickable(onClick = onClick).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, tint = c.icon, modifier = Modifier.size(22.dp))
        Spacer(Modifier.width(21.dp))
        Text(label, color = c.text, fontSize = 15.sp)
    }
}

/** The comment panel for a new note: "Add a comment" and Post. */
@Composable
fun NoteSheet(vm: AppViewModel, prompt: NotePrompt) {
    val at = prompt.at
    InlineNoteBar(
        hint = stringResource(R.string.viewer_write_note),
        onDismiss = { vm.notePrompt = null },
        onPost = { vm.postNote(prompt, it) },
        onVoice = if (at != null && prompt.selection == null) ({ vm.notePrompt = null; vm.startVoice(prompt.doc, at) }) else null,
    )
}

/** A reply being typed: the same bar, under the comment it answers. */
@Composable
fun ReplySheet(vm: AppViewModel, prompt: ReplyPrompt) {
    InlineNoteBar(hint = stringResource(R.string.viewer_write_reply), onDismiss = { vm.replyPrompt = null }, onPost = { vm.postReply(prompt, it) }, onVoice = null)
}

/**
 * The note bar the reference docks above the keyboard: up to three lines of text with no box,
 * then the microphone (record instead of typing, while nothing is typed) and, after a short
 * divider, the send arrow, blue once there is text. Back or a tap above it cancels.
 */
@Composable
private fun InlineNoteBar(hint: String, onDismiss: () -> Unit, onPost: (String) -> Unit, onVoice: (() -> Unit)?) {
    val c = pdfColors
    var text by remember { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    androidx.activity.compose.BackHandler(onBack = onDismiss)
    Box(Modifier.fillMaxSize()) {
        Box(Modifier.fillMaxSize().pointerInput(Unit) { detectTapGestures { onDismiss() } })
        Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().imePadding()
                .clip(RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp)).background(c.surface),
        ) {
            BasicTextField(
                value = text, onValueChange = { text = it }, maxLines = 3,
                textStyle = androidx.compose.ui.text.TextStyle(color = c.text, fontSize = 17.sp),
                cursorBrush = androidx.compose.ui.graphics.SolidColor(c.toolAccent),
                modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 15.dp, bottom = 15.dp).focusRequester(focus)
                    .semantics { contentDescription = hint },
                decorationBox = { inner ->
                    Box {
                        if (text.isEmpty()) Text(hint, color = c.textSecondary, fontSize = 17.sp)
                        inner()
                    }
                },
            )
            Row(Modifier.fillMaxWidth().height(48.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                if (onVoice != null) BarIcon(LucideIcons.Mic, stringResource(R.string.voice_record_comment), { onVoice() }, enabled = text.isBlank())
                Spacer(Modifier.weight(1f))
                Box(Modifier.width(1.dp).height(21.dp).background(c.divider))
                Spacer(Modifier.width(4.dp))
                BarIcon(LucideIcons.Send, stringResource(R.string.common_post), { onPost(text) }, enabled = text.isNotBlank(), tint = c.toolAccent)
            }
        }
    }
}

/** Typing free text where the page was tapped: a field over that spot, placed on Done. */
@Composable
fun BoxScope.TextEntry(vm: AppViewModel, prompt: TextPrompt, onDone: (TextPrompt, String) -> Unit) {
    val c = pdfColors
    var text by remember { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val doc = prompt.doc
    val at = doc.viewer.pageToScreen(prompt.at.page, prompt.at.x, prompt.at.y)
    val density = LocalDensity.current
    val color = vm.toolPrefs.color(StyledTool.FreeText)
    val sizeSp = with(density) { (vm.toolPrefs.textSize * doc.viewer.pixelsPerPoint(prompt.at.page)).toSp() }
    val keyboard = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current
    val currentText = androidx.compose.runtime.rememberUpdatedState(text)
    val placeholder = stringResource(R.string.viewer_add_text)
    // leaving the tool (Done) places the text too, after whatever else the tool had pending
    androidx.compose.runtime.DisposableEffect(prompt) {
        val before = doc.commitTool
        val commit = {
            before?.invoke()
            keyboard?.hide()
            onDone(prompt, currentText.value)
        }
        doc.commitTool = commit
        onDispose { if (doc.commitTool === commit) doc.commitTool = before }
    }
    // a tap anywhere else places the text (and puts the keyboard away)
    Box(Modifier.fillMaxSize().pointerInput(prompt) {
        detectTapGestures {
            keyboard?.hide()
            onDone(prompt, currentText.value)
        }
    })
    Box(Modifier.offset { IntOffset(at.x.roundToInt(), at.y.roundToInt()) }.imePadding()) {
        androidx.compose.foundation.text.BasicTextField(
            value = text,
            onValueChange = { text = it },
            textStyle = androidx.compose.ui.text.TextStyle(color = color, fontSize = sizeSp),
            cursorBrush = androidx.compose.ui.graphics.SolidColor(c.accent),
            modifier = Modifier
                .widthIn(min = 80.dp, max = 320.dp)
                .background(Color.White.copy(alpha = 0.6f))
                .padding(4.dp)
                .focusRequester(focus),
            decorationBox = { inner ->
                Box {
                    if (text.isEmpty()) Text(placeholder, color = color.copy(alpha = 0.5f), fontSize = sizeSp)
                    inner()
                }
            },
        )
    }
}

/** A hairline for menus. */
@Composable
fun MenuDivider() {
    Box(Modifier.fillMaxWidth().height(1.dp).background(pdfColors.divider))
}

/**
 * The selection's two handles: teardrops hanging under its first and last glyph. Dragging one
 * moves that end glyph by glyph; the other end stays.
 */
@Composable
fun BoxScope.SelectionHandles(doc: OpenDocument, sel: Selection) {
    val page = sel.range.page.toInt()
    val density = LocalDensity.current
    val size = with(density) { 20.dp.toPx() }
    // the reference's handles: 20 dp drops in one blue in both themes
    val blue = Color(0xFF4285F4)
    val startLabel = stringResource(R.string.viewer_selection_start)
    val endLabel = stringResource(R.string.viewer_selection_end)
    for (start in listOf(true, false)) {
        val r = (if (start) sel.startRect else sel.endRect) ?: continue
        // the handle hangs from the line's bottom, at its outer edge
        val anchor = doc.viewer.pageToScreen(page, if (start) r[0] else r[2], r[3])
        val lineHeight = doc.viewer.pageToScreen(page, r[0], r[3]).y - doc.viewer.pageToScreen(page, r[0], r[1]).y
        var drag by remember(sel.range, start) { mutableStateOf(Offset.Zero) }
        Box(
            Modifier
                .offset { IntOffset((anchor.x - size / 2f).roundToInt(), anchor.y.roundToInt()) }
                .size(20.dp)
                .pointerInput(sel.range.page, start) {
                    detectDragGestures(
                        onDragEnd = { drag = Offset.Zero; doc.selection.magnifierAt = null },
                        onDragCancel = { drag = Offset.Zero; doc.selection.magnifierAt = null },
                    ) { change, d ->
                        change.consume()
                        drag += d
                        // aim at the middle of the line the handle is under
                        val target = Offset(anchor.x, anchor.y - lineHeight / 2f) + drag
                        doc.selection.magnifierAt = target
                        doc.viewer.screenToPage(target, nearest = true)?.let { doc.selection.dragHandle(start, it) }
                    }
                }
                .semantics { contentDescription = if (start) startLabel else endLabel },
        ) {
            androidx.compose.foundation.Canvas(Modifier.fillMaxSize()) {
                val rad = this.size.width / 2f
                drawCircle(blue, rad, Offset(rad, rad * 1.1f))
                // the point of the drop reaches up to the text
                drawRect(blue, topLeft = if (start) Offset(rad, 0f) else Offset(0f, 0f), size = androidx.compose.ui.geometry.Size(rad, rad))
            }
        }
    }
}

/** Single page mode's scrubber: a pill with the page number along the bottom; drag to go through the pages. */
@Composable
fun BoxScope.HorizontalScrubber(doc: OpenDocument, visible: Boolean, bottom: Dp, onDrag: () -> Unit, onTap: () -> Unit) {
    val c = pdfColors
    val v = doc.viewer
    val density = LocalDensity.current
    var dragging by remember { mutableStateOf(false) }
    var dragX by remember { mutableFloatStateOf(0f) }
    AnimatedVisibility(visible || dragging, modifier = Modifier.align(Alignment.BottomCenter), enter = fadeIn(tween(150)), exit = fadeOut(tween(300))) {
        BoxWithConstraints(Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = bottom + 12.dp).height(32.dp)) {
            val track = with(density) { maxWidth.toPx() }
            val pillW = with(density) { 48.dp.toPx() }
            val n = (doc.pageCount - 1).coerceAtLeast(1)
            val x = if (dragging) dragX else v.currentPage.toFloat() / n * (track - pillW)
            val pageOf = stringResource(R.string.viewer_page_of, v.currentPage + 1, doc.pageCount)
            Box(Modifier.align(Alignment.CenterStart).fillMaxWidth().height(2.dp).background(c.divider))
            Box(
                Modifier
                    .offset { IntOffset(x.roundToInt(), 0) }
                    .size(48.dp, 30.dp)
                    .shadow(2.dp, RoundedCornerShape(15.dp))
                    .background(c.surface, RoundedCornerShape(15.dp))
                    .pointerInput(doc) { detectTapGestures(onTap = { onTap() }) }
                    .pointerInput(doc) {
                        detectDragGestures(
                            onDragStart = { dragging = true; dragX = x },
                            onDragEnd = { dragging = false },
                            onDragCancel = { dragging = false },
                        ) { change, d ->
                            change.consume()
                            dragX = (dragX + d.x).coerceIn(0f, track - pillW)
                            val page = (dragX / (track - pillW).coerceAtLeast(1f) * n).roundToInt()
                            if (page != v.currentPage) v.goToPage(page)
                            onDrag()
                        }
                    }
                    .semantics { contentDescription = pageOf },
                contentAlignment = Alignment.Center,
            ) {
                Text("${v.currentPage + 1}", color = c.text, fontSize = 15.sp)
            }
        }
    }
}
