package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.pdfColors
import kotlin.math.roundToInt

/** A long press on an empty part of a page: the menu of what can start there. */
class PageMenu(val doc: OpenDocument, val at: PagePoint, val screen: Offset, val pageHasText: Boolean)

/**
 * The reference's page menu: a row of the commenting tools that start at the pressed point
 * (comment, draw, text, sign), then a list of page tools. Tapping outside closes it.
 */
@Composable
fun PageMenuView(vm: AppViewModel, menu: PageMenu) {
    val c = pdfColors
    val doc = menu.doc
    val density = LocalDensity.current
    var size by remember { mutableStateOf(IntSize.Zero) }
    val close = { vm.pageMenu = null }
    val annotate = doc.info.allowsAnnotation
    val fill = doc.info.allowsFormFilling || annotate
    Box(Modifier.fillMaxSize().pointerInput(Unit) { detectTapGestures { close() } }) {
        val margin = with(density) { 8.dp.toPx() }
        val x = (menu.screen.x - size.width / 2f).coerceIn(margin, (doc.viewer.viewW - size.width - margin).coerceAtLeast(margin))
        // below the finger when it fits, else above it
        val below = menu.screen.y + with(density) { 24.dp.toPx() }
        val y = if (below + size.height < doc.viewer.viewH - doc.viewer.padBottom) below else (menu.screen.y - size.height - with(density) { 24.dp.toPx() }).coerceAtLeast(doc.viewer.padTop)
        Column(
            Modifier
                .offset { IntOffset(x.roundToInt(), y.roundToInt()) }
                .onSizeChanged { size = it }
                .width(209.dp)
                .shadow(6.dp, RoundedCornerShape(4.dp))
                .background(c.menu, RoundedCornerShape(4.dp))
                .pointerInput(Unit) { detectTapGestures { } },
        ) {
            Row(Modifier.fillMaxWidth().height(61.dp), horizontalArrangement = Arrangement.SpaceEvenly, verticalAlignment = Alignment.CenterVertically) {
                PageMenuIcon(LucideIcons.MessageSquarePlus, stringResource(R.string.page_menu_add_comment), annotate) { close(); vm.notePrompt = NotePrompt(doc, menu.at) }
                PageMenuIcon(LucideIcons.Pencil, stringResource(R.string.common_tool_draw), annotate) { close(); vm.enterTool(doc, ViewerTool.Draw) }
                PageMenuIcon(LucideIcons.Type, stringResource(R.string.page_menu_add_text), annotate) { close(); vm.textPrompt = TextPrompt(doc, menu.at) }
                if (fill) PageMenuIcon(LucideIcons.Signature, stringResource(R.string.common_sign), true) { close(); vm.enterTool(doc, ViewerTool.FillSign) }
            }
            Box(Modifier.fillMaxWidth().padding(horizontal = 4.dp).height(1.dp).background(c.divider))
            Spacer(Modifier.height(8.dp))
            if (!menu.pageHasText) PageMenuRow(LucideIcons.ScanText, stringResource(R.string.common_tool_recognize_text)) { close(); vm.recognizeText(doc) }
            PageMenuRow(LucideIcons.Crop, stringResource(R.string.common_tool_crop)) { close(); vm.openCrop(doc) }
            PageMenuRow(LucideIcons.FilePenLine, stringResource(R.string.common_tool_edit_pdf)) { close(); vm.enterTool(doc, ViewerTool.Edit) }
            PageMenuRow(LucideIcons.ImagePlus, stringResource(R.string.page_menu_add_image)) { close(); vm.addImageAt(doc, menu.at) }
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun PageMenuIcon(icon: ImageVector, label: String, enabled: Boolean, onClick: () -> Unit) {
    val c = pdfColors
    Box(
        Modifier.size(48.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick).semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, tint = if (enabled) c.icon else c.icon.copy(alpha = 0.35f), modifier = Modifier.size(24.dp))
    }
}

@Composable
private fun PageMenuRow(icon: ImageVector, label: String, onClick: () -> Unit) {
    val c = pdfColors
    Row(Modifier.fillMaxWidth().height(41.dp).clickable(onClick = onClick).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, tint = c.icon, modifier = Modifier.size(22.dp))
        Spacer(Modifier.width(21.dp))
        Text(label, color = c.text, fontSize = 15.sp)
    }
}
