package io.github.dikshant363.pdfcraft.ui.viewer

import android.content.Context
import androidx.annotation.StringRes
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.toMutableStateList
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlin.math.roundToInt

/**
 * A tool that can sit on the viewer's bottom toolbar or in More tools: [label] on the bar,
 * [title] in More tools, [subTools] when it opens a row of sub-tools (the bar's corner mark).
 */
enum class QuickTool(val id: String, @StringRes val label: Int, @StringRes val title: Int, val icon: ImageVector, val hue: Color, val subTools: Boolean = false) {
    Edit("edit", R.string.common_tool_edit_pdf, R.string.common_tool_edit_pdf, LucideIcons.FilePenLine, ToolHues.edit),
    Comment("comment", R.string.common_tool_comment, R.string.common_tool_comment, LucideIcons.MessageSquareText, ToolHues.comment, subTools = true),
    Highlight("highlight", R.string.common_tool_highlight, R.string.common_tool_highlight, LucideIcons.Highlighter, ToolHues.text, subTools = true),
    Draw("draw", R.string.common_tool_draw, R.string.common_tool_draw, LucideIcons.PenLine, ToolHues.sign),
    FillSign("fillsign", R.string.common_tool_fill_sign, R.string.common_tool_fill_sign, LucideIcons.Signature, ToolHues.sign, subTools = true),
    Export("export", R.string.common_tool_export, R.string.common_tool_export, LucideIcons.FileOutput, ToolHues.export),
    Combine("combine", R.string.toolbar_label_combine, R.string.common_tool_combine, LucideIcons.Combine, ToolHues.combine),
    Compress("compress", R.string.toolbar_label_compress, R.string.common_tool_compress, LucideIcons.FileArchive, ToolHues.compress),
    Protect("protect", R.string.toolbar_label_password, R.string.common_tool_set_password, LucideIcons.FileLock2, ToolHues.protect),
    Organize("organize", R.string.toolbar_label_organize, R.string.common_tool_organize, LucideIcons.LayoutGrid, ToolHues.organize),
    Crop("crop", R.string.toolbar_label_crop, R.string.common_tool_crop, LucideIcons.Crop, ToolHues.crop),
    Ocr("ocr", R.string.toolbar_label_recognize, R.string.common_tool_recognize_text, LucideIcons.ScanText, ToolHues.ocr),
    Create("create", R.string.common_tool_create_pdf, R.string.common_tool_create_pdf, LucideIcons.FilePlus2, ToolHues.create),
    Text("text", R.string.toolbar_label_add_text, R.string.toolbar_tool_text, LucideIcons.Type, ToolHues.text),
    Scan("scan", R.string.common_tool_scan, R.string.common_tool_scan, LucideIcons.Camera, ToolHues.scan),
    ReadAloud("readaloud", R.string.common_tool_read_aloud, R.string.common_tool_read_aloud, LucideIcons.Volume2, ToolHues.read);

    companion object {
        /** The bar as it comes: the reference's default tools and order. */
        val DEFAULT_BAR = listOf(Edit, Comment, Highlight, Draw, FillSign, Export)

        fun of(id: String): QuickTool? = entries.firstOrNull { it.id == id }
    }
}

/** How many tools fit on the bar beside More tools at a screen [widthDp] wide. */
fun toolbarSlots(widthDp: Int): Int = when {
    widthDp >= 768 -> 8
    widthDp >= 460 -> 7
    else -> 6
}

/** The bar's tools and More tools' order, as the user arranged them (kept in the app's preferences). */
@Stable
class QuickToolbarPrefs(context: Context) {
    private val prefs = context.getSharedPreferences("quick_toolbar", Context.MODE_PRIVATE)

    var bar by mutableStateOf(read("bar") ?: QuickTool.DEFAULT_BAR)
        private set

    /** More tools' order: every tool not on the bar. */
    var grid by mutableStateOf(complete(read("grid").orEmpty(), bar))
        private set

    private fun read(key: String): List<QuickTool>? =
        prefs.getString(key, null)?.let { csv -> csv.split(',').filter { it.isNotBlank() }.mapNotNull(QuickTool::of).distinct() }

    fun save(newBar: List<QuickTool>, newGrid: List<QuickTool>) {
        bar = newBar.distinct()
        grid = complete(newGrid, bar)
        prefs.edit().putString("bar", bar.joinToString(",") { it.id }).putString("grid", grid.joinToString(",") { it.id }).apply()
    }

    companion object {
        /** [order] without the bar's tools, followed by any tool it is missing. */
        fun complete(order: List<QuickTool>, bar: List<QuickTool>): List<QuickTool> {
            val g = order.filter { it !in bar }.distinct()
            return g + QuickTool.entries.filter { it !in bar && it !in g }
        }
    }
}

/** Do what [tool] does on the bar or in More tools. */
fun AppViewModel.runQuickTool(doc: OpenDocument, tool: QuickTool) {
    when (tool) {
        QuickTool.Edit -> enterTool(doc, ViewerTool.Edit)
        QuickTool.Comment -> enterTool(doc, ViewerTool.Comment)
        QuickTool.Highlight -> enterTool(doc, ViewerTool.Markup)
        QuickTool.Draw -> enterTool(doc, ViewerTool.Draw)
        QuickTool.FillSign -> enterTool(doc, ViewerTool.FillSign)
        QuickTool.Text -> enterTool(doc, ViewerTool.Text)
        QuickTool.Export -> openExport(doc)
        QuickTool.Combine -> openCombine(doc)
        QuickTool.Compress -> openCompress(doc)
        QuickTool.Protect -> openProtect(doc)
        QuickTool.Organize -> {
            doc.controller.pagesOpen = true
            organizing = true
        }
        QuickTool.Crop -> openCrop(doc)
        QuickTool.Ocr -> recognizeText(doc)
        QuickTool.Create -> openCreate()
        QuickTool.Scan -> startScan()
        QuickTool.ReadAloud -> startReadAloud(doc)
    }
}

/**
 * Customize toolbar: the bar's tools on top and the rest below, as More tools holds them. A tool
 * is dragged (or tapped) from one to the other; dropped on a full bar it pushes the bar's last
 * tool down. The tick keeps the new arrangement, the cross leaves the old one.
 */
@Composable
fun CustomizeToolbar(vm: AppViewModel, onDone: () -> Unit) {
    val c = pdfColors
    val prefs = vm.quickToolbar
    val slots = toolbarSlots(LocalConfiguration.current.screenWidthDp)
    val bar = remember { prefs.bar.take(slots).toMutableStateList() }
    val grid = remember { mutableStateListOf<QuickTool>().apply { addAll(QuickToolbarPrefs.complete(prefs.grid + prefs.bar.drop(slots), bar)) } }
    val rects = remember { mutableStateMapOf<QuickTool, Rect>() }
    var barRect by remember { mutableStateOf(Rect.Zero) }
    var gridRect by remember { mutableStateOf(Rect.Zero) }
    var origin by remember { mutableStateOf(Offset.Zero) }
    var dragging by remember { mutableStateOf<QuickTool?>(null) }
    var dragPos by remember { mutableStateOf(Offset.Zero) }

    fun toBar(t: QuickTool, index: Int) {
        bar.remove(t)
        grid.remove(t)
        bar.add(index.coerceIn(0, bar.size), t)
        while (bar.size > slots) grid.add(0, bar.removeAt(bar.lastIndex))
    }

    fun toGrid(t: QuickTool, index: Int) {
        bar.remove(t)
        grid.remove(t)
        grid.add(index.coerceIn(0, grid.size), t)
    }

    fun drop(t: QuickTool, at: Offset) {
        when {
            barRect.contains(at) -> toBar(t, bar.count { it != t && (rects[it]?.center?.x ?: 0f) < at.x })
            gridRect.contains(at) -> {
                // before the nearest tile, or after it when dropped on its right half
                val near = grid.filter { it != t }.minByOrNull { (rects[it]?.center ?: Offset.Infinite).minus(at).getDistanceSquared() }
                val others = grid.filter { it != t }
                val i = near?.let { n -> others.indexOf(n) + if (at.x > (rects[n]?.center?.x ?: 0f)) 1 else 0 } ?: others.size
                toGrid(t, i)
            }
        }
    }

    val wobble by rememberInfiniteTransition(label = "jiggle").animateFloat(-1.5f, 1.5f, infiniteRepeatable(tween(140), RepeatMode.Reverse), label = "angle")

    @Composable
    fun Draggable(t: QuickTool, content: @Composable () -> Unit) {
        val title = stringResource(t.title)
        val description = if (t in bar) stringResource(R.string.toolbar_tool_on_bar, title) else stringResource(R.string.toolbar_tool_in_more_tools, title)
        Box(
            Modifier
                .onGloballyPositioned { rects[t] = it.boundsInRoot() }
                .alpha(if (dragging == t) 0.3f else 1f)
                .rotate(if (dragging == null) wobble else 0f)
                .pointerInput(t) {
                    detectDragGestures(
                        onDragStart = { p ->
                            dragging = t
                            dragPos = (rects[t]?.topLeft ?: Offset.Zero) + p
                        },
                        onDrag = { change, amount ->
                            change.consume()
                            dragPos += amount
                        },
                        onDragEnd = {
                            drop(t, dragPos)
                            dragging = null
                        },
                        onDragCancel = { dragging = null },
                    )
                }
                .clickable(role = Role.Button) { if (t in bar) toGrid(t, 0) else toBar(t, bar.size) }
                .semantics { contentDescription = description },
        ) { content() }
    }

    Box(Modifier.fillMaxWidth().onGloballyPositioned { origin = it.boundsInRoot().topLeft }) {
        Column {
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.toolbar_customize), color = c.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                BarIcon(LucideIcons.X, stringResource(R.string.common_cancel), onDone)
                BarIcon(LucideIcons.Check, stringResource(R.string.toolbar_save), {
                    prefs.save(bar.toList(), grid.toList())
                    onDone()
                }, tint = c.accent)
            }
            Text(
                pluralStringResource(R.plurals.toolbar_customize_hint, slots, slots),
                color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
            )
            Text(stringResource(R.string.toolbar_section_toolbar), color = c.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, top = 12.dp, bottom = 6.dp))
            BoxWithConstraints(
                Modifier.fillMaxWidth().padding(horizontal = 8.dp).clip(RoundedCornerShape(8.dp)).background(c.pressed)
                    .onGloballyPositioned { barRect = it.boundsInRoot() },
            ) {
                val itemW = minOf(72.dp, (maxWidth - 8.dp) / (slots + 1))
                Row(Modifier.fillMaxWidth().heightIn(min = BottomBarHeight), verticalAlignment = Alignment.CenterVertically) {
                    for (t in bar) Draggable(t) { BarTool(t, itemW) }
                    Spacer(Modifier.weight(1f))
                    // More tools always ends the bar
                    Box(Modifier.alpha(0.5f)) { BarTool(null, itemW) }
                }
            }
            Text(stringResource(R.string.common_tool_more_tools), color = c.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, top = 16.dp, bottom = 6.dp))
            Column(Modifier.fillMaxWidth().padding(horizontal = 8.dp).heightIn(min = 96.dp).onGloballyPositioned { gridRect = it.boundsInRoot() }) {
                grid.chunked(4).forEach { row ->
                    Row(Modifier.fillMaxWidth()) {
                        row.forEach { t -> Box(Modifier.weight(1f), contentAlignment = Alignment.Center) { Draggable(t) { GridTool(t) } } }
                        repeat(4 - row.size) { Spacer(Modifier.weight(1f)) }
                    }
                    Spacer(Modifier.height(8.dp))
                }
            }
            Spacer(Modifier.height(16.dp))
        }
        // the tool under the finger
        val t = dragging
        if (t != null) {
            val half = with(LocalDensity.current) { 36.dp.toPx() }
            Box(
                Modifier.offset { IntOffset((dragPos.x - origin.x - half).roundToInt(), (dragPos.y - origin.y - half).roundToInt()) }
                    .shadow(6.dp, RoundedCornerShape(8.dp)).background(c.surface, RoundedCornerShape(8.dp)),
            ) { BarTool(t, 72.dp) }
        }
    }
}

/** A tool as the bar shows it: icon over its short label ([tool] null: More tools). */
@Composable
private fun BarTool(tool: QuickTool?, width: androidx.compose.ui.unit.Dp) {
    val c = pdfColors
    // grows with a large font setting, as the bar does
    Column(Modifier.width(width).heightIn(min = BottomBarHeight), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = androidx.compose.foundation.layout.Arrangement.Center) {
        Icon(tool?.icon ?: LucideIcons.LayoutGrid, null, tint = c.icon, modifier = Modifier.size(24.dp))
        Spacer(Modifier.height(6.dp))
        Text(
            stringResource(tool?.label ?: R.string.common_tool_more_tools), color = c.text, fontSize = 12.sp, lineHeight = 14.sp, maxLines = 2,
            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis, textAlign = TextAlign.Center,
        )
    }
}

/** A tool as More tools shows it: a tinted tile over its name. */
@Composable
private fun GridTool(tool: QuickTool) {
    val c = pdfColors
    Column(Modifier.width(80.dp).padding(vertical = 6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(48.dp).clip(RoundedCornerShape(4.dp)).background(tileBackground(tool.hue, c.dark)), contentAlignment = Alignment.Center) {
            Icon(tool.icon, null, tint = tool.hue, modifier = Modifier.size(26.dp))
        }
        Spacer(Modifier.height(8.dp))
        Text(stringResource(tool.title), color = c.text, fontSize = 13.sp, textAlign = TextAlign.Center, maxLines = 2, lineHeight = 16.sp)
    }
}
