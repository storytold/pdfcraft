package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.CommentInfo
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.Inter
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.pdfColors

/**
 * A selected comment's panel, as the reference's: "‹ N of M ›" to step through the document's
 * comment threads (and the colour button when the comment has one), the thread's comments with
 * their ⋮ menus (Copy text, Edit comment, Delete), and a reply box. It opens at 40% of the screen
 * and rises to 60% once its list scrolls.
 */
@Composable
fun CommentPanel(vm: AppViewModel, doc: OpenDocument, c: CommentInfo) {
    val colors = pdfColors
    var editing by remember(c.page, c.index) { mutableStateOf<CommentInfo?>(null) }
    var palette by remember(c.page, c.index) { mutableStateOf(false) }
    var reply by remember(c.page, c.index) { mutableStateOf("") }
    var menuFor by remember(c.page, c.index) { mutableStateOf<CommentInfo?>(null) }
    // the whole thread: replies to the comment and to its replies, in document order
    val replies = run {
        val names = mutableSetOf<String>().apply { c.name?.let(::add) }
        val out = mutableListOf<CommentInfo>()
        var grew = true
        while (grew) {
            grew = false
            for (r in doc.comments) {
                if (r.state == null && r.replyTo != null && r.replyTo in names && r !in out) {
                    out += r
                    r.name?.let(names::add)
                    grew = true
                }
            }
        }
        doc.comments.filter { it in out }
    }
    val thread = listOf(c) + replies
    // the threads the navigator steps through, in page order
    val threads = doc.comments.filter { it.replyTo == null && it.state == null && !it.fillSign && it.subtype in LISTED }
    val position = threads.indexOfFirst { it.page == c.page && it.index == c.index }
    fun go(to: CommentInfo) {
        val y = to.rect.getOrNull(1) ?: 0f
        doc.viewer.goToPage(to.page.toInt(), (y - 80f).coerceAtLeast(0f))
        vm.selectedComment = doc to to
    }
    val canEdit = !c.locked && doc.info.allowsAnnotation
    val snippets = remember(doc.info.generation) { androidx.compose.runtime.mutableStateMapOf<String, androidx.compose.ui.graphics.ImageBitmap>() }
    val scroll = rememberScrollState()
    var expanded by remember(c.page, c.index) { mutableStateOf(false) }
    LaunchedEffect(scroll.value > 0) { if (scroll.value > 0) expanded = true }
    val landscape = androidx.compose.ui.platform.LocalConfiguration.current.let { it.screenWidthDp > it.screenHeightDp }
    val fraction by androidx.compose.animation.core.animateFloatAsState(if (expanded) 0.6f else if (landscape) 0.5f else 0.4f, label = "panel")
    PdfSheet(onDismiss = { vm.selectedComment = null }, modifier = Modifier.imePadding(), maxHeightFraction = fraction, background = colors.menu, handleHeight = 12.dp, aboveNavBar = true, fill = true) {
        Column(Modifier.fillMaxWidth().fillMaxHeight()) {
            // the navigator, and the colour button at the right
            Box(Modifier.fillMaxWidth().height(40.dp)) {
                Row(Modifier.align(Alignment.Center), verticalAlignment = Alignment.CenterVertically) {
                    BarIcon(LucideIcons.ChevronLeft, stringResource(R.string.comments_previous), { threads.getOrNull(position - 1)?.let(::go) }, size = 40.dp, enabled = position > 0)
                    Spacer(Modifier.width(2.dp))
                    Text(
                        if (position >= 0) stringResource(R.string.comments_position, position + 1, threads.size) else stringResource(R.string.comments_position, 1, threads.size + 1),
                        color = colors.navigatorText, fontSize = 15.sp, modifier = Modifier.widthIn(min = 42.dp),
                        textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    )
                    Spacer(Modifier.width(2.dp))
                    BarIcon(LucideIcons.ChevronRight, stringResource(R.string.comments_next), { threads.getOrNull(position + 1)?.let(::go) }, size = 40.dp, enabled = position in 0 until threads.size - 1)
                }
                // a note has no property buttons; other comments get their colour here
                if (canEdit && c.color != null && c.subtype != "Text" && c.subtype != "Sound") {
                    Box(
                        Modifier.align(Alignment.CenterEnd).padding(end = 16.dp).size(40.dp)
                            .then(if (palette) Modifier.background(colors.pressed, RoundedCornerShape(5.dp)) else Modifier)
                            .clickable { palette = !palette },
                        contentAlignment = Alignment.Center,
                    ) {
                        Box(Modifier.size(20.dp).background(Color(0xFF000000 or c.color!!.toLong()), androidx.compose.foundation.shape.CircleShape))
                    }
                }
            }
            if (palette) {
                // the comment's colour, and as the reference also offers there its opacity and,
                // for drawings and shapes, its line width
                val shown = c.color?.let { Color(0xFF000000 or it.toLong()) } ?: Color.Transparent
                Spacer(Modifier.height(8.dp))
                ColorGrid(shown) { vm.recolorComment(doc, c, it) }
                if (c.subtype != "FreeText") {
                    var opacity by remember(c.page, c.index) { mutableStateOf(c.opacity) }
                    LaunchedEffect(opacity) {
                        if (opacity == c.opacity) return@LaunchedEffect
                        kotlinx.coroutines.delay(300)
                        vm.styleComment(doc, c, mapOf("opacity" to opacity.toDouble()))
                    }
                    Text(stringResource(R.string.comments_opacity), color = colors.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 78.dp, top = 16.dp, bottom = 8.dp))
                    OpacitySlider(shown, opacity) { opacity = it }
                }
                if (c.width != null && c.subtype in LineComments) {
                    Text(stringResource(R.string.comments_line_thickness), color = colors.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, top = 16.dp, bottom = 8.dp))
                    InkWidthRow(shown, InkWidths.minByOrNull { kotlin.math.abs(it - (c.width ?: 0f)) }) { vm.styleComment(doc, c, mapOf("width" to it.toDouble())) }
                }
                Spacer(Modifier.height(8.dp))
            }
            Column(Modifier.weight(1f).verticalScroll(scroll)) {
                thread.forEachIndexed { i, cm ->
                    if (editing?.let { it.page == cm.page && it.index == cm.index } == true) {
                        var text by remember(cm.page, cm.index) { mutableStateOf(cm.contents.orEmpty()) }
                        Column(Modifier.padding(start = 56.dp, end = 16.dp, top = 8.dp)) {
                            OutlinedTextField(
                                value = text, onValueChange = { text = it }, modifier = Modifier.fillMaxWidth().heightIn(min = 80.dp),
                                colors = OutlinedTextFieldDefaults.colors(focusedBorderColor = colors.toolAccent, cursorColor = colors.toolAccent),
                            )
                            Row {
                                Spacer(Modifier.weight(1f))
                                TextButton(onClick = { editing = null }) { Text(stringResource(R.string.common_cancel), color = colors.toolAccent) }
                                TextButton(onClick = { editing = null; vm.editCommentText(doc, cm, text) }) { Text(stringResource(R.string.common_save), color = colors.toolAccent, fontWeight = FontWeight.SemiBold) }
                            }
                        }
                    } else {
                        CommentRow(
                            vm, doc, cm,
                            reply = i > 0,
                            lineAbove = i > 0,
                            lineBelow = i < thread.size - 1,
                            replyLink = false,
                            lastOnPage = false,
                            snippet = snippets["${cm.page}-${cm.index}"],
                            onSnippet = { snippets["${cm.page}-${cm.index}"] = it },
                            onOpen = { go(c) },
                            onReply = {},
                            onMenu = { menuFor = cm },
                            panel = true,
                            menu = {
                                DropdownMenu(expanded = menuFor === cm, onDismissRequest = { menuFor = null }, containerColor = colors.menu) {
                                    val text = cm.contents?.takeIf { it.isNotBlank() }
                                    if (text != null) {
                                        DropdownMenuItem(
                                            text = { Text(stringResource(R.string.comments_copy_text), color = colors.text) },
                                            leadingIcon = { Icon(LucideIcons.Copy, null, tint = colors.icon) },
                                            onClick = { menuFor = null; vm.copyText(text) },
                                        )
                                    }
                                    if (!cm.locked && doc.info.allowsAnnotation) {
                                        DropdownMenuItem(
                                            text = { Text(stringResource(if (text == null && i == 0) R.string.comments_add else R.string.comments_edit), color = colors.text) },
                                            leadingIcon = { Icon(LucideIcons.Pencil, null, tint = colors.icon) },
                                            onClick = { menuFor = null; editing = cm },
                                        )
                                        DropdownMenuItem(
                                            text = { Text(stringResource(R.string.common_delete), color = colors.text) },
                                            leadingIcon = { Icon(LucideIcons.Trash2, null, tint = colors.icon) },
                                            onClick = { menuFor = null; vm.deleteComment(doc, cm) },
                                        )
                                    }
                                }
                            },
                        )
                    }
                }
                Spacer(Modifier.height(4.dp))
            }
            if (canEdit) {
                // the reply box: an outlined field with the send arrow after a short divider
                Row(
                    Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 12.dp).heightIn(min = 40.dp)
                        .border(1.dp, colors.replyBorder, RoundedCornerShape(5.dp)).padding(start = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    BasicTextField(
                        value = reply, onValueChange = { reply = it }, maxLines = 3,
                        textStyle = TextStyle(fontFamily = Inter, fontSize = 16.sp, color = colors.text),
                        cursorBrush = SolidColor(colors.toolAccent),
                        modifier = Modifier.weight(1f).padding(vertical = 10.dp),
                        decorationBox = { inner ->
                            Box {
                                if (reply.isEmpty()) Text(stringResource(R.string.comments_reply_hint), color = colors.replyBorder, fontSize = 16.sp)
                                inner()
                            }
                        },
                    )
                    if (reply.isNotEmpty()) {
                        Box(Modifier.padding(horizontal = 4.dp).size(width = 1.dp, height = 21.dp).background(colors.divider))
                        BarIcon(LucideIcons.Send, stringResource(R.string.common_post), { vm.postReply(ReplyPrompt(doc, c), reply); reply = "" }, size = 40.dp, enabled = reply.isNotBlank(), tint = colors.toolAccent)
                    }
                }
            }
        }
    }
}

/** Comments drawn with a line whose width can be changed. */
private val LineComments = setOf("Ink", "Line", "Square", "Circle", "Polygon", "PolyLine")
