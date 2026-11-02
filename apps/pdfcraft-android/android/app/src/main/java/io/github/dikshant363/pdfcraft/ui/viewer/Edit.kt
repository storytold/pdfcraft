package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Icon
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.ime
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.foundation.layout.widthIn
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.res.stringResource
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlin.math.roundToInt

/** A paragraph Edit PDF can change: its number on the page, text, box (points) and style. */
data class Paragraph(
    val page: Int,
    val number: Int,
    val text: String,
    val box: FloatArray,
    val font: String,
    val size: Float,
    val bold: Boolean,
    val italic: Boolean,
    val color: String?,
)

/** A picture Edit PDF has selected: its number on the page and where it sits (points). */
data class EditImage(val page: Int, val number: Int, val rect: FloatArray)

/** What Edit PDF is doing: picking a paragraph, or placing new text or an image. */
enum class EditMode { Select, AddText, AddImage }

/** The selection outline and its handles, as the reference draws them. */
private val SelectionBlue = Color(0xFF448AFF)

/** The text colours Edit PDF offers, in the reference's order. */
private val EditColors = listOf(
    "#ff0000" to R.string.prefs_color_red, "#ffff00" to R.string.prefs_color_yellow, "#0000ff" to R.string.prefs_color_blue,
    "#00ff00" to R.string.prefs_color_light_green, "#9900ff" to R.string.prefs_color_purple, "#00ffff" to R.string.prefs_color_cyan,
    "#009900" to R.string.prefs_color_green, "#ffffff" to R.string.prefs_color_white, "#000000" to R.string.prefs_color_black,
)

/** Which formatting panel is open over the bar. */
private enum class FormatPanel { None, Size, Font, Color, Align, List }

/**
 * Edit PDF's bottom bar. With nothing selected: Organize pages, then Add text and Add image. With a
 * paragraph selected (or being typed in), its formatting: bold, italic, underline, size, font,
 * colour and alignment, each change applied to the page at once.
 */
@Composable
fun EditToolbar(vm: AppViewModel, doc: OpenDocument) {
    val para = vm.editSelection
    if (para != null) EditFormatBar(vm, doc, para) else EditToolsBar(vm, doc)
}

@Composable
private fun EditToolsBar(vm: AppViewModel, doc: OpenDocument) {
    Row(
        // taller when a large font setting enlarges the labels, rather than cutting them off
        Modifier.fillMaxWidth().heightIn(min = BottomBarHeight).barTopLine().background(pdfColors.surface),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val textHint = stringResource(R.string.edit_tap_for_text)
        val imageHint = stringResource(R.string.edit_tap_for_image)
        EditBarItem(LucideIcons.LayoutGrid, stringResource(R.string.common_tool_organize), false, 96.dp) {
            vm.organizing = true
            doc.controller.pagesOpen = true
        }
        BarDividerLine()
        EditBarItem(LucideIcons.Type, stringResource(R.string.edit_add_text), vm.editMode == EditMode.AddText, 70.dp) {
            vm.editMode = if (vm.editMode == EditMode.AddText) EditMode.Select else EditMode.AddText
            if (vm.editMode == EditMode.AddText) vm.tell(textHint)
        }
        // as Add text: a mode, then a tap places the picture there
        EditBarItem(LucideIcons.ImagePlus, stringResource(R.string.edit_add_image), vm.editMode == EditMode.AddImage, 78.dp) {
            vm.editMode = if (vm.editMode == EditMode.AddImage) EditMode.Select else EditMode.AddImage
            if (vm.editMode == EditMode.AddImage) vm.tell(imageHint)
        }
    }
}

@Composable
private fun BarDividerLine() {
    Box(Modifier.padding(horizontal = 10.dp).width(1.dp).height(30.dp).background(pdfColors.divider))
}

@Composable
private fun EditBarItem(icon: ImageVector, label: String, selected: Boolean, width: Dp, onClick: () -> Unit) {
    val c = pdfColors
    Column(
        Modifier.width(width).heightIn(min = BottomBarHeight).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = label },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(icon, null, tint = if (selected) c.toolAccent else c.icon, modifier = Modifier.size(24.dp))
        Spacer(Modifier.height(6.dp))
        Text(label, color = if (selected) c.toolAccent else c.label, fontSize = 13.sp, maxLines = 1, softWrap = false)
    }
}

@Composable
private fun EditFormatBar(vm: AppViewModel, doc: OpenDocument, para: Paragraph) {
    val c = pdfColors
    var panel by remember(para.page, para.number) { mutableStateOf(FormatPanel.None) }
    fun toggle(p: FormatPanel) {
        panel = if (panel == p) FormatPanel.None else p
    }
    Column(Modifier.fillMaxWidth().background(c.surface)) {
        when (panel) {
            FormatPanel.Size -> SizePanel(para) { vm.styleParagraph(doc, para, mapOf("size" to it.toDouble())) }
            FormatPanel.Font -> FontPanel(vm.editFont[para.page to para.number]) { f ->
                vm.editFont[para.page to para.number] = f
                vm.styleParagraph(doc, para, mapOf("font" to f, "bold" to para.bold, "italic" to para.italic))
            }
            FormatPanel.Color -> ColorPanel(para.color) { vm.styleParagraph(doc, para, mapOf("color" to it)) }
            FormatPanel.Align -> AlignPanel(vm.editAlign[para.page to para.number] ?: "left") { a ->
                vm.editAlign[para.page to para.number] = a
                vm.styleParagraph(doc, para, mapOf("align" to a))
            }
            FormatPanel.List -> ListPanel(ListStyle.of(vm.editDraft?.takeIf { vm.editing != null } ?: para.text)) { vm.listParagraph(doc, para, it) }
            FormatPanel.None -> Unit
        }
        Row(
            Modifier.fillMaxWidth().height(BottomBarHeight).barTopLine(),
            horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val underline = vm.editUnderline[para.page to para.number] == true
            // the reference draws these as serif T's: heavy, slanted, underlined, and a small t beside a big one
            LetterButton("T", stringResource(R.string.edit_bold), para.bold, weight = FontWeight.Black) { vm.styleParagraph(doc, para, mapOf("bold" to !para.bold)) }
            LetterButton("T", stringResource(R.string.edit_italic), para.italic, italic = true) { vm.styleParagraph(doc, para, mapOf("italic" to !para.italic)) }
            LetterButton("T", stringResource(R.string.edit_underline), underline, underline = true) {
                vm.editUnderline[para.page to para.number] = !underline
                vm.styleParagraph(doc, para, mapOf("underline" to !underline))
            }
            LetterButton("tT", stringResource(R.string.edit_text_size), panel == FormatPanel.Size, small = true) { toggle(FormatPanel.Size) }
            val fontLabel = stringResource(R.string.edit_font)
            Box(
                Modifier.size(44.dp).clickable(role = Role.Button) { toggle(FormatPanel.Font) }.semantics { contentDescription = fontLabel },
                contentAlignment = Alignment.Center,
            ) { Text("Ag", color = if (panel == FormatPanel.Font) c.toolAccent else c.icon, fontSize = 20.sp, fontFamily = FontFamily.Serif) }
            val colourLabel = stringResource(R.string.edit_colour)
            Box(
                Modifier.size(44.dp).clickable(role = Role.Button) { toggle(FormatPanel.Color) }.semantics { contentDescription = colourLabel },
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier.size(24.dp).background(parseHex(para.color ?: "#000000"), CircleShape)
                        .then(if (panel == FormatPanel.Color) Modifier.drawBehind { drawCircle(c.toolAccent, size.minDimension / 2 + 3.dp.toPx(), style = Stroke(2.dp.toPx())) } else Modifier),
                )
            }
            FormatButton(alignIcon(vm.editAlign[para.page to para.number] ?: "left"), stringResource(R.string.edit_align_text), true, tintSelected = panel == FormatPanel.Align || true) { toggle(FormatPanel.Align) }
            // bulleted and numbered lists, as the reference's phone toolbar offers
            FormatButton(LucideIcons.List, stringResource(R.string.edit_list), true, tintSelected = true) { toggle(FormatPanel.List) }
        }
    }
}

@Composable
private fun LetterButton(
    letters: String,
    description: String,
    selected: Boolean,
    weight: FontWeight = FontWeight.SemiBold,
    italic: Boolean = false,
    underline: Boolean = false,
    small: Boolean = false,
    onClick: () -> Unit,
) {
    val c = pdfColors
    Box(
        Modifier.size(44.dp).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Text(
            letters, color = if (selected) c.toolAccent else c.icon, fontSize = if (small) 19.sp else 24.sp, fontFamily = FontFamily.Serif, fontWeight = weight,
            fontStyle = if (italic) FontStyle.Italic else FontStyle.Normal, textDecoration = if (underline) TextDecoration.Underline else null,
        )
    }
}

@Composable
private fun FormatButton(icon: ImageVector, description: String, selected: Boolean, tintSelected: Boolean = selected, onClick: () -> Unit) {
    val c = pdfColors
    Box(
        Modifier.size(44.dp).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { Icon(icon, null, tint = if (tintSelected) c.toolAccent else c.icon, modifier = Modifier.size(26.dp)) }
}

/** Text size: "Text size: N pt" over a stepped slider between a small and a large A. */
@Composable
private fun SizePanel(para: Paragraph, onSize: (Float) -> Unit) {
    val c = pdfColors
    var size by remember(para.page, para.number, para.size) { mutableFloatStateOf(para.size.coerceIn(6f, 66f)) }
    Column(Modifier.fillMaxWidth().barTopLine().padding(top = 10.dp, bottom = 6.dp)) {
        Text(stringResource(R.string.edit_text_size_pt, size.roundToInt()), color = c.label, fontSize = 15.sp, textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth())
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(horizontal = 8.dp)) {
            Text("A", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 10.dp))
            Slider(
                value = size, onValueChange = { size = ((it / 2f).roundToInt() * 2f).coerceIn(6f, 66f) }, onValueChangeFinished = { onSize(size) },
                valueRange = 6f..66f, steps = 29,
                colors = SliderDefaults.colors(thumbColor = Color.White, activeTrackColor = c.textSecondary, inactiveTrackColor = c.textSecondary, activeTickColor = c.textSecondary, inactiveTickColor = c.textSecondary),
                modifier = Modifier.weight(1f),
            )
            Text("A", color = c.textSecondary, fontSize = 22.sp, modifier = Modifier.padding(horizontal = 10.dp))
        }
    }
}

/** The faces the engine can set text in, each named in its own face; the paragraph's own comes first. */
@Composable
private fun FontPanel(current: String?, onFont: (String) -> Unit) {
    val c = pdfColors
    val faces = listOf(null to R.string.edit_font_original, "helvetica" to R.string.edit_font_sans, "times" to R.string.edit_font_serif, "courier" to R.string.edit_font_mono)
    LazyColumn(Modifier.fillMaxWidth().heightIn(max = 240.dp).barTopLine().padding(horizontal = 24.dp, vertical = 4.dp)) {
        items(faces.size) { i ->
            val (key, name) = faces[i]
            val chosen = key == current
            Row(
                Modifier.fillMaxWidth().height(42.dp).clickable(enabled = key != null) { if (key != null) onFont(key) },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    stringResource(name), color = if (key == null) c.textSecondary else c.label, fontSize = 17.sp, modifier = Modifier.weight(1f),
                    fontFamily = when (key) { "times" -> FontFamily.Serif; "courier" -> FontFamily.Monospace; else -> FontFamily.SansSerif },
                )
                if (chosen) Icon(LucideIcons.Check, null, tint = c.textSecondary, modifier = Modifier.size(22.dp))
            }
        }
    }
}

/** Nine colour swatches across the screen, the current one ticked. */
@Composable
private fun ColorPanel(current: String?, onColor: (String) -> Unit) {
    Row(Modifier.fillMaxWidth().barTopLine().padding(horizontal = 12.dp, vertical = 12.dp)) {
        for ((hex, name) in EditColors) {
            val col = parseHex(hex)
            val label = stringResource(name)
            Box(
                Modifier.weight(1f).height(40.dp).background(col).clickable { onColor(hex) }.semantics { contentDescription = label },
                contentAlignment = Alignment.Center,
            ) {
                if (current.equals(hex, ignoreCase = true)) Icon(LucideIcons.Check, null, tint = if (hex == "#ffffff" || hex == "#ffff00" || hex == "#00ffff" || hex == "#00ff00") Color.Black else Color.White, modifier = Modifier.size(22.dp))
            }
        }
    }
}

/** None, the bullets and the numberings, the paragraph's own one marked. */
@Composable
private fun ListPanel(current: ListStyle, onPick: (ListStyle) -> Unit) {
    val c = pdfColors
    Row(Modifier.fillMaxWidth().barTopLine().padding(vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterHorizontally)) {
        for (style in ListStyle.entries) {
            val description = stringResource(listDescription(style))
            Box(
                Modifier.height(40.dp).widthIn(min = 40.dp).clickable { onPick(style) }.padding(horizontal = 6.dp).semantics { contentDescription = description },
                contentAlignment = Alignment.Center,
            ) { Text(style.label, color = if (style == current) c.toolAccent else c.icon, fontSize = 17.sp, fontWeight = if (style == current) FontWeight.SemiBold else FontWeight.Normal) }
        }
    }
}

@Composable
private fun AlignPanel(current: String, onAlign: (String) -> Unit) {
    val c = pdfColors
    Row(Modifier.fillMaxWidth().barTopLine().padding(vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterHorizontally)) {
        for (a in listOf("left", "right", "center", "justify")) {
            val description = stringResource(alignDescription(a))
            Box(
                Modifier.size(40.dp).clickable { onAlign(a) }.semantics { contentDescription = description },
                contentAlignment = Alignment.Center,
            ) { Icon(alignIcon(a), null, tint = if (a == current) c.toolAccent else c.icon, modifier = Modifier.size(24.dp)) }
        }
    }
}

/**
 * Edit PDF over the pages: every paragraph outlined in dashes; the selected one in solid blue with
 * corner handles, a menu over it (Edit text, Select all, Delete), dragged to move it or by its
 * handles to change its width; the paragraph being typed in, edited where it sits; and a selected
 * picture with its menu (turn left, turn right, delete), dragged to move or by a corner to resize.
 */
@Composable
fun BoxScope.EditOverlay(vm: AppViewModel, doc: OpenDocument) {
    val pages = doc.viewer.visiblePages()
    LaunchedEffect(pages, doc.info.generation) { for (p in pages) vm.loadParagraphs(doc, p) }
    val accent = pdfColors.accent
    val selected = vm.editSelection
    val editing = vm.editing
    Canvas(Modifier.fillMaxSize()) {
        val dash = PathEffect.dashPathEffect(floatArrayOf(8f, 6f))
        // with a paragraph or picture selected, the reference shows only that one's outline
        if (selected == null && vm.editImage == null) for (p in pages) {
            for (para in vm.paragraphs[p].orEmpty()) {
                val a = doc.viewer.pageToScreen(p, para.box[0], para.box[1])
                val b = doc.viewer.pageToScreen(p, para.box[2], para.box[3])
                drawRect(accent.copy(alpha = 0.6f), topLeft = Offset(a.x - 3f, a.y - 3f), size = Size(b.x - a.x + 6f, b.y - a.y + 6f), style = Stroke(width = 1.dp.toPx(), pathEffect = dash))
            }
        }
    }
    if (selected != null) {
        if (editing != null && editing.page == selected.page && editing.number == selected.number) InlineEditor(vm, doc, editing) else SelectedParagraph(vm, doc, selected)
    }
    vm.editImage?.let { SelectedImage(vm, doc, it) }
}

/** A box on screen for [box] (points) on [page], with the drag offset applied. */
private fun screenBox(doc: OpenDocument, page: Int, box: FloatArray, pad: Float): Pair<Offset, Offset> {
    val a = doc.viewer.pageToScreen(page, box[0], box[1])
    val b = doc.viewer.pageToScreen(page, box[2], box[3])
    return Offset(a.x - pad, a.y - pad) to Offset(b.x + pad, b.y + pad)
}

@Composable
private fun BoxScope.SelectedParagraph(vm: AppViewModel, doc: OpenDocument, para: Paragraph) {
    val density = LocalDensity.current
    val pad = with(density) { 3.dp.toPx() }
    val (a0, b0) = screenBox(doc, para.page, para.box, pad)
    // a drag in progress: the move, or the width change from either side
    var move by remember(para) { mutableStateOf(Offset.Zero) }
    var grow by remember(para) { mutableFloatStateOf(0f) }
    var growLeft by remember(para) { mutableStateOf(false) }
    val a = Offset(a0.x + move.x + if (growLeft) grow else 0f, a0.y + move.y)
    val b = Offset(b0.x + move.x + if (growLeft) 0f else grow, b0.y + move.y)
    val ppp = doc.viewer.pixelsPerPoint(para.page).coerceAtLeast(0.01f)
    Canvas(Modifier.fillMaxSize()) {
        drawRect(SelectionBlue, topLeft = a, size = Size(b.x - a.x, b.y - a.y), style = Stroke(2.dp.toPx()))
        for (h in listOf(a, Offset(b.x, a.y), b, Offset(a.x, b.y))) {
            drawCircle(Color.White, 7.dp.toPx(), h)
            drawCircle(SelectionBlue, 7.dp.toPx(), h, style = Stroke(2.dp.toPx()))
        }
    }
    // the body: tap to type in it, drag to move it
    with(density) {
        Box(
            Modifier.offset { IntOffset(a.x.roundToInt(), a.y.roundToInt()) }.size((b.x - a.x).toDp(), (b.y - a.y).toDp())
                .pointerInput(para) { detectTapGestures { vm.startInlineEdit(para, selectAll = false) } }
                .pointerInput(para) {
                    detectDragGestures(
                        onDragEnd = {
                            val m = move
                            move = Offset.Zero
                            if (m.getDistance() > 2f) vm.moveParagraph(doc, para, m.x / ppp, m.y / ppp)
                        },
                        onDragCancel = { move = Offset.Zero },
                    ) { change, d ->
                        change.consume()
                        move += d
                    }
                },
        )
        // the four corners change the width (the left ones from the left)
        for ((corner, left) in listOf(a to true, Offset(b.x, a.y) to false, b to false, Offset(a.x, b.y) to true)) {
            Box(
                Modifier.offset { IntOffset((corner.x - 22.dp.toPx()).roundToInt(), (corner.y - 22.dp.toPx()).roundToInt()) }.size(44.dp)
                    .pointerInput(para, left) {
                        detectDragGestures(
                            onDragStart = { growLeft = left },
                            onDragEnd = {
                                val g = grow
                                grow = 0f
                                val w = (para.box[2] - para.box[0]) + (if (left) -g else g) / ppp
                                if (kotlin.math.abs(g) > 2f && w > 12f) vm.resizeParagraph(doc, para, w, if (left) g / ppp else 0f)
                            },
                            onDragCancel = { grow = 0f },
                        ) { change, d ->
                            change.consume()
                            grow += d.x
                        }
                    },
            )
        }
    }
    if (move == Offset.Zero && grow == 0f) {
        FloatingMenu(a, b, doc.viewer.padTop) {
            MenuText(stringResource(R.string.edit_edit_text)) { vm.startInlineEdit(para, selectAll = false) }
            MenuSplit()
            MenuText(stringResource(R.string.edit_select_all)) { vm.startInlineEdit(para, selectAll = true) }
            MenuSplit()
            MenuText(stringResource(R.string.common_delete)) { vm.deleteParagraph(doc, para) }
        }
    }
}

/** The dark menu the reference floats over a selection: centred above it, or below when there's no room. */
@Composable
private fun BoxScope.FloatingMenu(a: Offset, b: Offset, top: Float, content: @Composable () -> Unit) {
    val density = LocalDensity.current
    var menu by remember { mutableStateOf(IntSize.Zero) }
    val gap = with(density) { 12.dp.toPx() }
    val x = ((a.x + b.x) / 2f - menu.width / 2f).coerceAtLeast(with(density) { 8.dp.toPx() })
    val above = a.y - gap - menu.height
    val y = if (above > top) above else b.y + gap
    Row(
        Modifier.offset { IntOffset(x.roundToInt(), y.roundToInt()) }.onSizeChanged { menu = it }
            .shadow(6.dp, RoundedCornerShape(4.dp)).background(Color(0xFF303030), RoundedCornerShape(4.dp)).height(48.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) { content() }
}

@Composable
private fun MenuText(label: String, onClick: () -> Unit) {
    Box(Modifier.fillMaxHeightRow().clickable(onClick = onClick).padding(horizontal = 14.dp), contentAlignment = Alignment.Center) {
        Text(label, color = Color.White, fontSize = 15.sp)
    }
}

@Composable
private fun MenuIconButton(icon: ImageVector, label: String, onClick: () -> Unit) {
    Box(Modifier.size(48.dp).clickable(onClick = onClick).semantics { contentDescription = label }, contentAlignment = Alignment.Center) {
        Icon(icon, null, tint = Color.White, modifier = Modifier.size(24.dp))
    }
}

@Composable
private fun MenuSplit() {
    Box(Modifier.width(1.dp).height(48.dp).background(Color(0xFF4B4B4B)))
}

private fun Modifier.fillMaxHeightRow(): Modifier = this.height(48.dp)

/**
 * Typing in a paragraph where it sits: its text in its colour, size and face over the page's own
 * colour, outlined in blue. Tapping elsewhere or Done writes it into the page.
 */
@Composable
private fun BoxScope.InlineEditor(vm: AppViewModel, doc: OpenDocument, para: Paragraph) {
    val density = LocalDensity.current
    val (a, b) = screenBox(doc, para.page, para.box, with(density) { 3.dp.toPx() })
    val ppp = doc.viewer.pixelsPerPoint(para.page)
    val focus = remember { FocusRequester() }
    var value by remember(para) {
        mutableStateOf(TextFieldValue(para.text, if (vm.editSelectAll) TextRange(0, para.text.length) else TextRange(para.text.length)))
    }
    LaunchedEffect(para) { runCatching { focus.requestFocus() } }
    val ime = androidx.compose.foundation.layout.WindowInsets.ime.getBottom(density)
    LaunchedEffect(ime) {
        if (ime <= 0) return@LaunchedEffect
        val v = doc.viewer
        // the formatting bar (and a panel) sit on the keyboard: keep the box's bottom clear of them
        val clear = v.viewH - ime - with(density) { (BottomBarHeight + 64.dp).toPx() }
        if (b.y > clear) v.setScroll(v.scrollX, v.scrollY + (b.y - clear))
    }
    LaunchedEffect(value.text) { vm.editDraft = value.text }
    // misspelled words, checked a moment after typing stops, underlined in red (as the reference does)
    val context = androidx.compose.ui.platform.LocalContext.current
    val speller = remember { SpellCheck(context) }
    var typos by remember(para) { mutableStateOf<List<Typo>>(emptyList()) }
    var layout by remember { mutableStateOf<androidx.compose.ui.text.TextLayoutResult?>(null) }
    LaunchedEffect(value.text) {
        kotlinx.coroutines.delay(600)
        typos = speller.check(value.text)
    }
    var paper by remember(para) { mutableStateOf<Color?>(null) }
    LaunchedEffect(para) { paper = vm.paperColor(doc, para) }
    val paragraphLabel = stringResource(R.string.edit_paragraph_text)
    val fontPx = para.size * ppp
    val sp = with(density) { (fontPx / fontScale).toSp() }
    val family = when (vm.editFont[para.page to para.number] ?: baseFont(para.font)) {
        "times" -> FontFamily.Serif
        "courier" -> FontFamily.Monospace
        else -> FontFamily.SansSerif
    }
    with(density) {
        Box(
            Modifier.offset { IntOffset(a.x.roundToInt(), a.y.roundToInt()) }.width((b.x - a.x).toDp()).heightIn(min = (b.y - a.y).toDp())
                .background(paper ?: Color.White).drawBehind { drawRect(SelectionBlue, size = size, style = Stroke(2.dp.toPx())) }
                .padding(3.dp),
        ) {
            BasicTextField(
                value = value,
                onValueChange = { value = it },
                textStyle = TextStyle(
                    color = parseHex(para.color ?: "#000000"), fontSize = sp, lineHeight = sp * 1.2f, fontFamily = family,
                    fontWeight = if (para.bold) FontWeight.Bold else FontWeight.Normal,
                    fontStyle = if (para.italic) FontStyle.Italic else FontStyle.Normal,
                    textDecoration = if (vm.editUnderline[para.page to para.number] == true) TextDecoration.Underline else null,
                ),
                cursorBrush = SolidColor(SelectionBlue),
                onTextLayout = { layout = it },
                modifier = Modifier.fillMaxWidth().focusRequester(focus).semantics { contentDescription = paragraphLabel }
                    .drawWithContent {
                        drawContent()
                        val l = layout ?: return@drawWithContent
                        for (t in typos) {
                            if (t.end > l.layoutInput.text.length) continue
                            for (line in l.getLineForOffset(t.start)..l.getLineForOffset(t.end - 1)) {
                                val x0 = if (line == l.getLineForOffset(t.start)) l.getHorizontalPosition(t.start, true) else l.getLineLeft(line)
                                val x1 = if (line == l.getLineForOffset(t.end - 1)) l.getHorizontalPosition(t.end, true) else l.getLineRight(line)
                                drawSquiggle(x0, x1, l.getLineBottom(line) - 1.dp.toPx())
                            }
                        }
                    },
            )
        }
        // the word at the cursor, if misspelled: its suggestions above the box, a tap puts one in
        val at = value.selection.start
        val typo = typos.firstOrNull { value.selection.collapsed && at in it.start..it.end && it.suggestions.isNotEmpty() }
        if (typo != null) {
            Row(
                Modifier.offset { IntOffset(a.x.roundToInt(), (a.y - 48.dp.toPx()).roundToInt()) }
                    .background(Color(0xFF2C2C2C), androidx.compose.foundation.shape.RoundedCornerShape(4.dp)).padding(horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                for (word in typo.suggestions) {
                    Text(
                        word, color = Color.White, fontSize = 15.sp,
                        modifier = Modifier.clickable {
                            val t = value.text.replaceRange(typo.start, typo.end, word)
                            value = TextFieldValue(t, TextRange(typo.start + word.length))
                        }.padding(horizontal = 12.dp, vertical = 10.dp),
                    )
                }
            }
        }
    }
}

/** A red wavy line from [x0] to [x1] at [y], under a misspelled word. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawSquiggle(x0: Float, x1: Float, y: Float) {
    val step = 2.dp.toPx()
    val path = androidx.compose.ui.graphics.Path().apply {
        moveTo(x0, y)
        var x = x0
        var up = true
        while (x < x1) {
            val nx = minOf(x + step, x1)
            lineTo(nx, if (up) y - step / 1.5f else y)
            x = nx
            up = !up
        }
    }
    drawPath(path, Color(0xFFE34850), style = Stroke(1.dp.toPx()))
}

@Composable
private fun BoxScope.SelectedImage(vm: AppViewModel, doc: OpenDocument, img: EditImage) {
    val density = LocalDensity.current
    val (a0, b0) = screenBox(doc, img.page, img.rect, 0f)
    var move by remember(img) { mutableStateOf(Offset.Zero) }
    var scale by remember(img) { mutableFloatStateOf(1f) }
    val w0 = b0.x - a0.x
    val h0 = b0.y - a0.y
    val a = a0 + move
    val b = Offset(a.x + w0 * scale, a.y + h0 * scale)
    val ppp = doc.viewer.pixelsPerPoint(img.page).coerceAtLeast(0.01f)
    Canvas(Modifier.fillMaxSize()) {
        drawRect(SelectionBlue, topLeft = a, size = Size(b.x - a.x, b.y - a.y), style = Stroke(2.dp.toPx()))
        for (h in listOf(a, Offset(b.x, a.y), b, Offset(a.x, b.y))) {
            drawCircle(Color.White, 7.dp.toPx(), h)
            drawCircle(SelectionBlue, 7.dp.toPx(), h, style = Stroke(2.dp.toPx()))
        }
    }
    with(density) {
        Box(
            Modifier.offset { IntOffset(a.x.roundToInt(), a.y.roundToInt()) }.size((b.x - a.x).toDp(), (b.y - a.y).toDp())
                .pointerInput(img) {
                    detectDragGestures(
                        onDragEnd = {
                            val m = move
                            move = Offset.Zero
                            if (m.getDistance() > 2f) {
                                val dx = m.x / ppp
                                val dy = m.y / ppp
                                vm.placeImage(doc, img, floatArrayOf(img.rect[0] + dx, img.rect[1] + dy, img.rect[2] + dx, img.rect[3] + dy))
                            }
                        },
                        onDragCancel = { move = Offset.Zero },
                    ) { change, d ->
                        change.consume()
                        move += d
                    }
                },
        )
        // the lower right corner resizes, keeping the picture's shape
        Box(
            Modifier.offset { IntOffset((b.x - 22.dp.toPx()).roundToInt(), (b.y - 22.dp.toPx()).roundToInt()) }.size(44.dp)
                .pointerInput(img) {
                    detectDragGestures(
                        onDragEnd = {
                            val s = scale
                            scale = 1f
                            if (kotlin.math.abs(s - 1f) > 0.02f) {
                                val w = (img.rect[2] - img.rect[0]) * s
                                val h = (img.rect[3] - img.rect[1]) * s
                                vm.placeImage(doc, img, floatArrayOf(img.rect[0], img.rect[1], img.rect[0] + w, img.rect[1] + h))
                            }
                        },
                        onDragCancel = { scale = 1f },
                    ) { change, d ->
                        change.consume()
                        scale = ((w0 * scale + d.x) / w0).coerceIn(0.1f, 10f)
                    }
                },
        )
    }
    if (move == Offset.Zero && scale == 1f) {
        FloatingMenu(a, b, doc.viewer.padTop) {
            MenuIconButton(LucideIcons.RotateCcw, stringResource(R.string.edit_turn_left)) { vm.turnImage(doc, img, 3) }
            MenuIconButton(LucideIcons.RotateCw, stringResource(R.string.edit_turn_right)) { vm.turnImage(doc, img, 1) }
            MenuIconButton(LucideIcons.Trash2, stringResource(R.string.common_delete)) { vm.deleteImage(doc, img) }
        }
    }
}

/** What a screen reader says for each list style's button. */
private fun listDescription(style: ListStyle): Int = when (style) {
    ListStyle.None -> R.string.edit_list_desc_none
    ListStyle.Bullet -> R.string.edit_list_desc_bullet
    ListStyle.Dash -> R.string.edit_list_desc_dash
    ListStyle.Number -> R.string.edit_list_desc_number
    ListStyle.Lower -> R.string.edit_list_desc_lower
    ListStyle.Upper -> R.string.edit_list_desc_upper
    ListStyle.Roman -> R.string.edit_list_desc_roman
}

/** What a screen reader says for each alignment's button. */
private fun alignDescription(a: String): Int = when (a) {
    "center" -> R.string.edit_align_center
    "right" -> R.string.edit_align_right
    "justify" -> R.string.edit_align_justify
    else -> R.string.edit_align_left
}

private fun alignIcon(a: String) = when (a) {
    "center" -> LucideIcons.AlignCenter
    "right" -> LucideIcons.AlignRight
    "justify" -> LucideIcons.AlignJustify
    else -> LucideIcons.AlignLeft
}

/** The nearest of the engine's three faces to a PDF font name. */
fun baseFont(name: String): String {
    val n = name.lowercase()
    return when {
        "courier" in n || "mono" in n || "consol" in n -> "courier"
        "times" in n || "serif" in n && "sans" !in n || "georgia" in n || "garamond" in n || "minion" in n -> "times"
        else -> "helvetica"
    }
}

fun parseHex(s: String): Color = runCatching { Color(android.graphics.Color.parseColor(s)) }.getOrDefault(Color.Black)
