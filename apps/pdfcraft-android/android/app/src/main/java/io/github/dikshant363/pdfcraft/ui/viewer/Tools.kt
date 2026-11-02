package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import io.github.dikshant363.pdfcraft.ui.LocalUndo
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.res.stringResource
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.text
import io.github.dikshant363.pdfcraft.core.TextRange
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.SheetHost
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** A stroke being drawn: its page and points (in the page's points). */
class InkStroke(val page: Int) {
    val points = mutableStateListOf<Offset>()

    /** The clean shape shown while the finger is held still (press and hold to snap). */
    var preview by mutableStateOf<List<Offset>?>(null)
}

/** How long the finger stays still to snap a stroke to a shape, and how far it may wander. */
private const val SNAP_HOLD_MS = 500L
private val SNAP_SLOP = 8.dp

/** The text being marked with a markup tool: the page and the glyph range so far, and its rectangles. */
data class MarkupDrag(val range: TextRange, val rects: List<FloatArray>)

/**
 * The touch layer of the active tool over the pages. One finger acts (draws, marks text, places a
 * comment); two fingers still scroll and zoom the page, as in the reference.
 */
@Composable
fun ToolLayer(vm: AppViewModel, doc: OpenDocument) {
    val tool = doc.controller.tool
    if (tool == ViewerTool.None || tool == ViewerTool.Edit) return
    when (tool) {
        ViewerTool.Draw -> DrawLayer(vm, doc)
        ViewerTool.Markup -> MarkupLayer(vm, doc)
        ViewerTool.Comment -> TapLayer(doc) { p ->
            if (doc.controller.commentMode == CommentMode.Voice) vm.startVoice(doc, p) else vm.notePrompt = NotePrompt(doc, p)
        }
        ViewerTool.Text -> TapLayer(doc) { p -> vm.textPrompt = TextPrompt(doc, p) }
        ViewerTool.FillSign -> if (vm.certPlacing === doc) CertPlaceLayer(vm, doc) else FillSignLayer(vm, doc)
        else -> Unit
    }
}

/**
 * The gesture split every tool layer shares: [onOne] gets a one-finger gesture (down, moves, up,
 * or cancelled when a second finger joins), two fingers pan and zoom the view.
 */
fun Modifier.toolGestures(
    doc: OpenDocument,
    onDown: (Offset) -> Unit,
    onMove: (Offset) -> Unit,
    onUp: (Offset, moved: Boolean) -> Unit,
    onCancel: () -> Unit,
): Modifier = pointerInput(doc) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        var one = true
        var moved = false
        var last = down.position
        onDown(down.position)
        while (true) {
            val event = awaitPointerEvent()
            val pressed = event.changes.filter { it.pressed }
            if (pressed.isEmpty()) break
            if (pressed.size > 1) {
                if (one) {
                    one = false
                    onCancel()
                }
                val zoom = event.calculateZoom()
                if (zoom != 1f) doc.viewer.zoomBy(zoom, event.calculateCentroid())
                val pan = event.calculatePan()
                if (pan != Offset.Zero) doc.viewer.scrollBy(pan.x, pan.y)
                event.changes.forEach { if (it.positionChanged()) it.consume() }
                continue
            }
            if (!one) continue
            val c = pressed.first()
            if (c.positionChanged()) {
                if ((c.position - down.position).getDistance() > viewConfiguration.touchSlop) moved = true
                last = c.position
                onMove(c.position)
                c.consume()
            }
        }
        if (one) onUp(last, moved)
    }
}

/** A tool that acts on a tap (Comment, Text): two fingers scroll, a tap on a page calls [onTap]. */
@Composable
private fun TapLayer(doc: OpenDocument, onTap: (PagePoint) -> Unit) {
    Box(
        Modifier.fillMaxSize().toolGestures(
            doc,
            onDown = {},
            onMove = { },
            onUp = { p, moved -> if (!moved) doc.viewer.screenToPage(p)?.let(onTap) },
            onCancel = {},
        ),
    )
}

@Composable
private fun DrawLayer(vm: AppViewModel, doc: OpenDocument) {
    val strokes = remember { mutableStateListOf<InkStroke>() }
    // strokes taken back, for redo; a new stroke clears them
    val undone = remember { mutableStateListOf<InkStroke>() }
    var current by remember { mutableStateOf<InkStroke?>(null) }
    val prefs = vm.toolPrefs
    val color = prefs.color(StyledTool.Ink)
    val opacity = prefs.opacity(StyledTool.Ink)
    val width = prefs.inkWidth
    // what is drawn is added as one ink comment per page when the tool is left, in the
    // colour and width set then
    DisposableEffect(doc) {
        doc.commitTool = {
            val color = prefs.color(StyledTool.Ink)
            val opacity = prefs.opacity(StyledTool.Ink)
            val width = prefs.inkWidth
            val byPage = strokes.filter { it.points.size >= 2 }.groupBy { it.page }
            for ((page, list) in byPage) {
                vm.runTool(
                    doc, "comment_add",
                    mapOf(
                        "page" to page + 1, "type" to "ink",
                        "strokes" to list.map { s -> s.points.map { listOf(it.x.toDouble(), it.y.toDouble()) } },
                        "color" to color.hex(), "opacity" to opacity.toDouble(), "width" to width.toDouble(),
                        "author" to prefs.author.ifBlank { null },
                    ),
                )
            }
            strokes.clear()
            undone.clear()
        }
        doc.localUndo = LocalUndo(
            canUndo = { strokes.isNotEmpty() },
            canRedo = { undone.isNotEmpty() },
            undo = { if (strokes.isNotEmpty()) undone.add(strokes.removeAt(strokes.lastIndex)) },
            redo = { if (undone.isNotEmpty()) strokes.add(undone.removeAt(undone.lastIndex)) },
        )
        onDispose {
            doc.localUndo = null
        }
    }
    // press and hold: where the finger has been still since, and when it last moved further
    var stillAt by remember { mutableStateOf(Offset.Zero) }
    var movedAt by remember { mutableStateOf(0L) }
    val slop = with(LocalDensity.current) { SNAP_SLOP.toPx() }
    LaunchedEffect(current) {
        val s = current ?: return@LaunchedEffect
        while (true) {
            delay(50)
            if (s.preview == null && System.currentTimeMillis() - movedAt >= SNAP_HOLD_MS) {
                s.preview = ShapeSnap.recognise(s.points.toList())?.outline()
            }
        }
    }
    Box(
        Modifier.fillMaxSize().toolGestures(
            doc,
            onDown = { p ->
                val at = doc.viewer.screenToPage(p) ?: return@toolGestures
                stillAt = p
                movedAt = System.currentTimeMillis()
                current = InkStroke(at.page).also { it.points.add(Offset(at.x, at.y)); strokes.add(it) }
                undone.clear()
            },
            onMove = { p ->
                val s = current ?: return@toolGestures
                if ((p - stillAt).getDistance() > slop) {
                    stillAt = p
                    movedAt = System.currentTimeMillis()
                    s.preview = null
                }
                val at = doc.viewer.screenToPage(p, nearest = true) ?: return@toolGestures
                if (at.page == s.page) s.points.add(Offset(at.x, at.y))
            },
            onUp = { _, _ ->
                val s = current
                current = null
                if (s != null) {
                    // a held shape always snaps; with Snap to shape on, any stroke that is one does
                    val shape = s.preview ?: if (prefs.snapToShape) ShapeSnap.recognise(s.points.toList())?.outline() else null
                    s.preview = null
                    if (shape != null) {
                        s.points.clear()
                        s.points.addAll(shape)
                    }
                }
            },
            onCancel = {
                current?.let { strokes.remove(it) }
                current = null
            },
        ),
    ) {
        Canvas(Modifier.fillMaxSize()) {
            for (s in strokes) {
                val pts = s.preview ?: s.points
                if (pts.isEmpty()) continue
                val path = Path()
                pts.forEachIndexed { i, pt ->
                    val o = doc.viewer.pageToScreen(s.page, pt.x, pt.y)
                    if (i == 0) path.moveTo(o.x, o.y) else path.lineTo(o.x, o.y)
                }
                drawPath(
                    path, color.copy(alpha = opacity),
                    style = Stroke(width = width * doc.viewer.pixelsPerPoint(s.page), cap = StrokeCap.Round, join = StrokeJoin.Round),
                )
            }
        }
    }
}

@Composable
private fun MarkupLayer(vm: AppViewModel, doc: OpenDocument) {
    val scope = rememberCoroutineScope()
    var drag by remember { mutableStateOf<MarkupDrag?>(null) }
    var anchor by remember { mutableStateOf<Pair<Int, UInt>?>(null) }
    var job by remember { mutableStateOf<Job?>(null) }
    val eng = vm.engine ?: return
    val markup = doc.controller.markup
    val color = vm.toolPrefs.color(markup.styled)
    val opacity = vm.toolPrefs.opacity(markup.styled)

    fun extendTo(p: Offset) {
        val (page, a) = anchor ?: return
        val at = doc.viewer.screenToPage(p, nearest = true) ?: return
        if (at.page != page) return
        job?.cancel()
        job = scope.launch {
            val g = withContext(eng.commands) { eng.session.nearestGlyph(doc.id, page.toUInt(), at.x, at.y) } ?: return@launch
            val s = minOf(a, g)
            val e = maxOf(a, g) + 1u
            val range = TextRange(page.toUInt(), s, e)
            val rects = withContext(eng.commands) { eng.session.textRects(doc.id, range) }
            drag = MarkupDrag(range, rects.toList().chunked(4).filter { it.size == 4 }.map { it.toFloatArray() })
        }
    }

    Box(
        Modifier.fillMaxSize().toolGestures(
            doc,
            onDown = { p ->
                val at = doc.viewer.screenToPage(p) ?: return@toolGestures
                scope.launch {
                    val n = withContext(eng.commands) { eng.session.glyphCount(doc.id, at.page.toUInt()) }
                    if (n == 0u) {
                        vm.tell(text(R.string.toolbar_no_text_to_mark))
                        return@launch
                    }
                    val g = withContext(eng.commands) { eng.session.nearestGlyph(doc.id, at.page.toUInt(), at.x, at.y) } ?: return@launch
                    anchor = at.page to g
                }
            },
            onMove = { p -> extendTo(p) },
            onUp = { _, _ ->
                val d = drag
                drag = null
                anchor = null
                if (d != null && d.rects.isNotEmpty()) {
                    // the sub-tool and its colour as they are now, not when the layer was made
                    val m = doc.controller.markup
                    vm.addMarkup(doc, m, d.range.page.toInt(), d.rects, vm.toolPrefs.color(m.styled), vm.toolPrefs.opacity(m.styled))
                }
            },
            onCancel = {
                drag = null
                anchor = null
            },
        ),
    ) {
        Canvas(Modifier.fillMaxSize()) {
            val d = drag ?: return@Canvas
            val page = d.range.page.toInt()
            for (r in d.rects) {
                val a = doc.viewer.pageToScreen(page, r[0], r[1])
                val b = doc.viewer.pageToScreen(page, r[2], r[3])
                when (markup) {
                    TextMarkup.Highlight -> drawRect(color.copy(alpha = 0.4f * opacity), a, Size(b.x - a.x, b.y - a.y))
                    TextMarkup.Underline -> drawLine(color.copy(alpha = opacity), Offset(a.x, b.y - 1f), Offset(b.x, b.y - 1f), 2.dp.toPx())
                    TextMarkup.Strikethrough -> drawLine(color.copy(alpha = opacity), Offset(a.x, (a.y + b.y) / 2f), Offset(b.x, (a.y + b.y) / 2f), 2.dp.toPx())
                }
            }
        }
    }
}

/** Which property picker a sub-toolbar has open. */
enum class Picker { None, Color, Width, TextSize }

/** The active tool's sub-toolbar. */
@Composable
fun ToolSubToolbar(vm: AppViewModel, doc: OpenDocument) {
    val ctl = doc.controller
    val prefs = vm.toolPrefs
    val picker = ctl.picker
    fun toggle(p: Picker) {
        ctl.picker = if (ctl.picker == p) Picker.None else p
    }
    when (ctl.tool) {
        ViewerTool.Markup -> SubToolbar {
            SubTool(LucideIcons.Highlighter, stringResource(R.string.common_tool_highlight), ctl.markup == TextMarkup.Highlight, { ctl.markup = TextMarkup.Highlight })
            SubTool(LucideIcons.Underline, stringResource(R.string.common_tool_underline), ctl.markup == TextMarkup.Underline, { ctl.markup = TextMarkup.Underline })
            SubTool(LucideIcons.Strikethrough, stringResource(R.string.toolbar_strikethrough), ctl.markup == TextMarkup.Strikethrough, { ctl.markup = TextMarkup.Strikethrough })
            SubDivider()
            ColorDot(prefs.color(ctl.markup.styled), picker == Picker.Color) { toggle(Picker.Color) }
        }
        ViewerTool.Draw -> SubToolbar {
            val snapOn = stringResource(R.string.toolbar_snap_on)
            val snapOff = stringResource(R.string.toolbar_snap_off)
            SubTool(LucideIcons.PenLine, stringResource(R.string.toolbar_draw_freeform), true, {})
            SubDivider()
            SubProperty(LucideIcons.Menu, stringResource(R.string.toolbar_line_thickness), picker == Picker.Width, { toggle(Picker.Width) })
            ColorDot(prefs.color(StyledTool.Ink), picker == Picker.Color) { toggle(Picker.Color) }
            SubDivider()
            SubTool(LucideIcons.Shapes, stringResource(R.string.toolbar_snap_to_shape), prefs.snapToShape, {
                prefs.updateSnapToShape(!prefs.snapToShape)
                vm.tell(if (prefs.snapToShape) snapOn else snapOff)
            })
        }
        ViewerTool.Comment -> SubToolbar {
            SubTool(LucideIcons.MessageSquareText, stringResource(R.string.toolbar_text_comment), ctl.commentMode == CommentMode.Text, { ctl.commentMode = CommentMode.Text })
            SubTool(LucideIcons.MicAudioLines, stringResource(R.string.toolbar_voice_comment), ctl.commentMode == CommentMode.Voice, { ctl.commentMode = CommentMode.Voice })
        }
        ViewerTool.Text -> SubToolbar {
            SubTool(LucideIcons.Type, stringResource(R.string.toolbar_tool_text), true, {})
            SubDivider()
            SubProperty(LucideIcons.ALargeSmall, stringResource(R.string.toolbar_text_size), picker == Picker.TextSize, { toggle(Picker.TextSize) })
            ColorDot(prefs.color(StyledTool.FreeText), picker == Picker.Color) { toggle(Picker.Color) }
        }
        ViewerTool.FillSign -> FillSignToolbar(vm, doc)
        ViewerTool.Edit -> EditToolbar(vm, doc)
        ViewerTool.None -> Unit
    }
}

/**
 * The active tool's sheets (colour, thickness, text size; Fill & Sign's Sign panel), drawn over
 * the whole viewer rather than inside the bottom bar.
 */
@Composable
fun ToolSheets(vm: AppViewModel, doc: OpenDocument) {
    val ctl = doc.controller
    val styled = when (ctl.tool) {
        ViewerTool.Markup -> ctl.markup.styled
        ViewerTool.Draw -> StyledTool.Ink
        ViewerTool.Text -> StyledTool.FreeText
        else -> null
    }
    SheetHost(ctl.picker.takeIf { it != Picker.None && styled != null }) { p ->
        PropertyPicker(vm, styled ?: StyledTool.Highlight, p, ctl.markup) { ctl.picker = Picker.None }
    }
    if (ctl.tool == ViewerTool.FillSign && ctl.signPanel) SignPanel(vm, doc) { ctl.signPanel = false }
}

/** The colour, thickness or text-size picker of a tool, as a sheet above its sub-toolbar. */
@Composable
fun PropertyPicker(vm: AppViewModel, tool: StyledTool, picker: Picker, markup: TextMarkup, onDismiss: () -> Unit) {
    val prefs = vm.toolPrefs
    val c = pdfColors
    // over the tool's bar, which stays in view with the picker's button shaded
    PdfSheet(onDismiss = onDismiss, modifier = Modifier.padding(bottom = BottomBarHeight + 1.dp).navigationBarsPadding(), bottomBand = false) {
        when (picker) {
            Picker.Color -> {
                val title = stringResource(
                    when (tool) {
                        StyledTool.Highlight -> R.string.toolbar_color_highlight
                        StyledTool.Underline -> R.string.toolbar_color_underline
                        StyledTool.Strikethrough -> R.string.toolbar_color_strikethrough
                        StyledTool.Ink -> R.string.toolbar_color_drawing
                        StyledTool.FreeText -> R.string.toolbar_color_text
                        StyledTool.Note -> R.string.toolbar_color_comment
                    },
                )
                Text(title, color = c.text, fontSize = 17.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, top = 3.dp, bottom = 18.dp))
                ColorGrid(prefs.color(tool)) { prefs.setColor(tool, it) }
                if (tool != StyledTool.FreeText) {
                    Text(stringResource(R.string.toolbar_opacity), color = c.text, fontSize = 15.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, modifier = Modifier.padding(start = 78.dp, top = 20.dp, bottom = 8.dp))
                    OpacitySlider(prefs.color(tool), prefs.opacity(tool)) { prefs.setOpacity(tool, it) }
                }
                Spacer(Modifier.height(30.dp))
            }
            Picker.Width -> {
                Text(stringResource(R.string.toolbar_line_thickness), color = c.text, fontSize = 17.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, bottom = 12.dp))
                InkWidthRow(prefs.color(StyledTool.Ink), prefs.inkWidth) { prefs.updateInkWidth(it) }
            }
            Picker.TextSize -> {
                Text(stringResource(R.string.toolbar_text_size), color = c.text, fontSize = 17.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, bottom = 4.dp))
                Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.toolbar_text_size_pt, prefs.textSize.toInt()), color = c.text, fontSize = 15.sp, modifier = Modifier.width(56.dp))
                    Slider(
                        value = prefs.textSize,
                        onValueChange = { v -> prefs.updateTextSize((Math.round(v / 2f) * 2f).coerceIn(6f, 22f)) },
                        valueRange = 6f..22f,
                        steps = TextSizes.size - 2,
                        colors = SliderDefaults.colors(thumbColor = c.accent, activeTrackColor = c.accent),
                        modifier = Modifier.weight(1f),
                    )
                }
                Spacer(Modifier.height(8.dp))
            }
            Picker.None -> Unit
        }
    }
}

/** The seven line widths as bars of growing thickness, the chosen one framed. */
@Composable
fun InkWidthRow(color: Color, selected: Float?, onPick: (Float) -> Unit) {
    val c = pdfColors
    Row(Modifier.fillMaxWidth().padding(bottom = 16.dp), horizontalArrangement = Arrangement.Center) {
        InkWidths.forEachIndexed { i, w ->
            val description = stringResource(R.string.toolbar_stroke_width, i + 1)
            Box(
                Modifier.size(48.dp)
                    .then(if (selected == w) Modifier.border(1.5.dp, c.accent, RoundedCornerShape(4.dp)) else Modifier)
                    .clickable(role = Role.RadioButton) { onPick(w) }
                    .semantics { contentDescription = description },
                contentAlignment = Alignment.Center,
            ) {
                Box(Modifier.width(28.dp).height((1 + i * 1.2f).dp).background(color))
            }
        }
    }
}

/** The 18 colour swatches: 32 dp rounded squares 18 dp apart, the chosen one ringed and checked. */
@Composable
fun ColorGrid(selected: Color, onPick: (Color) -> Unit) {
    val c = pdfColors
    val ring = c.toolAccent
    Column(Modifier.fillMaxWidth().padding(horizontal = 51.dp), verticalArrangement = Arrangement.spacedBy(18.dp)) {
        CommentPalette.chunked(6).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                row.forEach { (color, name) ->
                    val label = stringResource(name)
                    val isSel = color.toArgbInt() == selected.toArgbInt()
                    // only a swatch the colour of the sheet itself needs an outline to be seen
                    val outlined = kotlin.math.abs(color.luminance() - c.surface.luminance()) < 0.08f
                    Box(
                        Modifier.size(32.dp)
                            .drawBehind {
                                if (isSel) {
                                    val w = 2.dp.toPx()
                                    val out = 4.dp.toPx()
                                    drawRoundRect(
                                        ring, topLeft = Offset(-out + w / 2, -out + w / 2), size = Size(size.width + 2 * out - w, size.height + 2 * out - w),
                                        cornerRadius = androidx.compose.ui.geometry.CornerRadius(6.dp.toPx()), style = androidx.compose.ui.graphics.drawscope.Stroke(w),
                                    )
                                }
                            }
                            .clip(RoundedCornerShape(3.dp))
                            .background(color)
                            .then(if (outlined) Modifier.border(1.dp, c.divider, RoundedCornerShape(3.dp)) else Modifier)
                            .clickable(role = Role.RadioButton) { onPick(color) }
                            .semantics { contentDescription = label },
                        contentAlignment = Alignment.Center,
                    ) {
                        if (isSel) Icon(LucideIcons.Check, null, tint = if (color.luminance() > 0.5f) Color.Black else Color.White, modifier = Modifier.size(20.dp))
                    }
                }
            }
        }
    }
}

/**
 * Opacity from 25 % to 100 %: a 35 dp checkered track (12 dp squares) washed with the tool's colour,
 * and a round thumb in that colour, as the reference draws it. Tap or drag along it.
 */
@Composable
fun OpacitySlider(color: Color, value: Float, onChange: (Float) -> Unit) {
    val min = 0.25f
    val sheet = pdfColors.surface
    val description = stringResource(R.string.toolbar_opacity_percent, (value * 100).toInt())
    Box(
        Modifier.fillMaxWidth().padding(horizontal = 78.dp).height(35.dp)
            .pointerInput(Unit) {
                fun at(x: Float) = onChange((min + (x / size.width).coerceIn(0f, 1f) * (1f - min)))
                detectTapGestures { at(it.x) }
            }
            .pointerInput(Unit) {
                detectHorizontalDragGestures { change, _ -> onChange((min + (change.position.x / size.width).coerceIn(0f, 1f) * (1f - min))) }
            }
            .semantics { contentDescription = description },
    ) {
        Canvas(Modifier.fillMaxSize()) {
            val sq = 12.dp.toPx()
            clipPath(androidx.compose.ui.graphics.Path().apply {
                addRoundRect(androidx.compose.ui.geometry.RoundRect(0f, 0f, size.width, size.height, androidx.compose.ui.geometry.CornerRadius(4.dp.toPx())))
            }) {
                var y = 0f
                var r = 0
                while (y < size.height) {
                    var x = 0f
                    var col = 0
                    while (x < size.width) {
                        drawRect(if ((col + r) % 2 == 0) Color(0xFF8A8A8A) else Color(0xFFE6E6E6), Offset(x, y), Size(sq, sq))
                        x += sq
                        col++
                    }
                    y += sq
                    r++
                }
                drawRect(androidx.compose.ui.graphics.Brush.horizontalGradient(listOf(color.copy(alpha = min), color)), size = size)
            }
            val cx = (value - min) / (1f - min) * size.width
            val radius = 10.5.dp.toPx()
            // a thin white ring, a gap in the sheet's colour, then the colour itself
            drawCircle(Color.White, radius, Offset(cx, size.height / 2))
            drawCircle(sheet, radius - 1.dp.toPx(), Offset(cx, size.height / 2))
            drawCircle(color.copy(alpha = 1f), radius - 3.dp.toPx(), Offset(cx, size.height / 2))
        }
    }
}

private fun Color.luminance(): Float = 0.2126f * red + 0.7152f * green + 0.0722f * blue
