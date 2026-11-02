package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.activity.compose.BackHandler
import androidx.annotation.StringRes
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.CommentInfo
import io.github.dikshant363.pdfcraft.core.TileRect
import io.github.dikshant363.pdfcraft.engine.toBitmap
import io.github.dikshant363.pdfcraft.plural
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.SheetRow
import io.github.dikshant363.pdfcraft.ui.pdfColors
import io.github.dikshant363.pdfcraft.ui.safeTopPadding
import kotlinx.coroutines.withContext

/** The comment types the list shows: the commenting kinds, not attachments, links or pop-ups. */
internal val LISTED = setOf(
    "Text", "FreeText", "Highlight", "Underline", "StrikeOut", "Squiggly", "Ink", "Line", "Square", "Circle",
    "Polygon", "PolyLine", "Stamp", "Caret", "Sound",
)

/** The markups whose row shows a picture of the text they mark. */
private val SNIPPETS = setOf("Highlight", "Underline", "StrikeOut")

/** What a comment did, in the list's italic line (a string resource). */
@StringRes
internal fun commentActionLabel(c: CommentInfo): Int = when (c.subtype) {
    "Highlight" -> R.string.comments_action_highlighted
    "Underline" -> R.string.comments_action_underlined
    "StrikeOut" -> R.string.comments_action_struck_out
    "Ink" -> R.string.comments_action_freehand
    "Line" -> if (c.kind.contains("arrow", ignoreCase = true)) R.string.comments_action_arrow else R.string.comments_action_line
    "Square" -> R.string.comments_action_rectangle
    "Circle" -> R.string.comments_action_oval
    "Polygon" -> if (c.kind.contains("cloud", ignoreCase = true)) R.string.comments_action_cloud else R.string.comments_action_polygon
    "PolyLine" -> R.string.comments_action_joined_lines
    "Stamp" -> R.string.comments_action_stamp
    "Text" -> R.string.comments_action_note
    "FreeText" -> if (c.kind.contains("callout", ignoreCase = true)) R.string.comments_action_callout else R.string.comments_action_text_box
    "Caret" -> if (c.kind.contains("replace", ignoreCase = true)) R.string.comments_action_replacement else R.string.comments_action_insertion
    "Sound" -> R.string.comments_action_voice
    else -> R.string.comments_action_other
}

/** When a comment was made: [today] ("Today"), [yesterday] ("Yesterday"), else the date. */
internal fun commentDateLabel(raw: String?, today: String, yesterday: String, now: Long = System.currentTimeMillis()): String? {
    val t = pdfDateMillis(raw) ?: return null
    fun day(ms: Long) = java.util.Calendar.getInstance().apply { timeInMillis = ms }.let { it.get(java.util.Calendar.YEAR) * 400 + it.get(java.util.Calendar.DAY_OF_YEAR) }
    return when (day(t)) {
        day(now) -> today
        day(now - 24L * 3600 * 1000) -> yesterday
        else -> java.text.DateFormat.getDateInstance(java.text.DateFormat.MEDIUM).format(java.util.Date(t))
    }
}

/** "N comment threads" (replies aren't threads of their own): [none] for none, else [count] of them. */
internal fun threadsTitle(n: Int, none: String, count: (Int) -> String): String = if (n == 0) none else count(n)

/**
 * Comment List, full screen as the reference's is on a phone: "N comment threads", the threads
 * grouped under "Page N" headers that fold, each comment with its avatar, author, date, what it did
 * (or a picture of the marked text), its text and, under each thread, Reply. A tap goes to the
 * comment on its page.
 */
@Composable
fun CommentListScreen(vm: AppViewModel, doc: OpenDocument) {
    val c = pdfColors
    val ctl = doc.controller
    var list by remember { mutableStateOf<List<CommentInfo>?>(null) }
    LaunchedEffect(doc.info.generation) { list = vm.comments(doc) }
    val folded = remember { mutableStateMapOf<Int, Boolean>() }
    val snippets = remember(doc.info.generation) { mutableStateMapOf<String, ImageBitmap>() }
    var menuFor by remember { mutableStateOf<CommentInfo?>(null) }
    fun close() {
        ctl.sheet = ViewerSheet.None
    }
    BackHandler(onBack = ::close)
    val all = list.orEmpty().filter { !it.fillSign }
    val parents = all.filter { it.replyTo == null && it.state == null && it.subtype in LISTED }
    // each thread: its comment, then the replies to it in order
    val threads = parents.map { p -> listOf(p) + all.filter { it.replyTo != null && it.replyTo == p.name && it.state == null } }
    Column(
        Modifier.fillMaxSize().background(Color.Black).safeTopPadding()
            .background(c.navBand).navigationBarsPadding().background(c.surface),
    ) {
        Row(
            Modifier.fillMaxWidth().height(TopBarHeight).drawBehind {
                val line = 1.dp.toPx()
                drawRect(c.divider, Offset(0f, size.height - line), Size(size.width, line))
            }.padding(bottom = 1.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(56.dp), contentAlignment = Alignment.Center) {
                BarIcon(LucideIcons.ArrowLeft, stringResource(R.string.common_back), ::close, iconSize = 27.dp)
            }
            Text(
                if (list == null) stringResource(R.string.comments_title) else threadsTitle(threads.size, stringResource(R.string.comments_none)) { plural(R.plurals.comments_threads, it, it) },
                color = c.label, fontSize = 17.sp, fontWeight = FontWeight.SemiBold,
            )
        }
        if (list != null && threads.isEmpty()) {
            EmptyComments()
            return@Column
        }
        LazyColumn(Modifier.fillMaxSize()) {
            val pages = threads.groupBy { it.first().page.toInt() }
            pages.entries.forEachIndexed { pageIndex, (page, pageThreads) ->
                item(key = "p$page") {
                    val isFolded = folded[page] == true
                    Row(
                        Modifier.fillMaxWidth()
                            .drawBehind {
                                if (pageIndex > 0) {
                                    val inset = 16.dp.toPx()
                                    drawRect(c.divider, Offset(inset, 0f), Size(size.width - 2 * inset, 2.dp.toPx()))
                                }
                            }
                            .clickable { folded[page] = !isFolded }
                            .padding(start = 16.dp, top = if (pageIndex == 0) 12.dp else 15.dp, end = 16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(stringResource(R.string.sheet_page, page + 1), color = c.label, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        // the chevron's 40 dp target overlaps the rows around it rather than spacing them
                        Box(Modifier.size(width = 40.dp, height = 22.dp), contentAlignment = Alignment.Center) {
                            Icon(
                                LucideIcons.ChevronUp, stringResource(if (isFolded) R.string.comments_expand else R.string.comments_collapse), tint = c.icon,
                                modifier = Modifier.size(22.dp).rotate(if (isFolded) 180f else 0f),
                            )
                        }
                    }
                }
                if (folded[page] != true) {
                    pageThreads.forEachIndexed { threadIndex, thread ->
                        thread.forEachIndexed { i, cm ->
                            item(key = "c${cm.page}-${cm.index}") {
                                CommentRow(
                                    vm, doc, cm,
                                    reply = i > 0,
                                    lineAbove = i > 0,
                                    lineBelow = i < thread.size - 1,
                                    replyLink = i == thread.size - 1,
                                    lastOnPage = threadIndex == pageThreads.size - 1 && i == thread.size - 1,
                                    snippet = snippets["${cm.page}-${cm.index}"],
                                    onSnippet = { snippets["${cm.page}-${cm.index}"] = it },
                                    onOpen = {
                                        val parent = thread.first()
                                        close()
                                        val y = parent.rect.getOrNull(1) ?: 0f
                                        doc.viewer.goToPage(parent.page.toInt(), (y - 40f).coerceAtLeast(0f))
                                        vm.selectedComment = doc to parent
                                    },
                                    onReply = { vm.replyPrompt = ReplyPrompt(doc, thread.first()) },
                                    onMenu = { menuFor = cm },
                                )
                            }
                        }
                    }
                }
            }
        }
    }
    menuFor?.let { cm ->
        PdfSheet(onDismiss = { menuFor = null }, dim = true) {
            val text = cm.contents?.takeIf { it.isNotBlank() }
            if (text != null) {
                SheetRow(LucideIcons.Copy, stringResource(R.string.comments_copy_text), {
                    menuFor = null
                    vm.copyText(text)
                })
            }
            if (text != null && !cm.locked) {
                SheetRow(LucideIcons.Pencil, stringResource(R.string.comments_edit), {
                    menuFor = null
                    vm.selectedComment = doc to cm
                })
            }
            if (!cm.locked) {
                SheetRow(LucideIcons.Trash2, stringResource(R.string.common_delete), {
                    menuFor = null
                    vm.deleteComment(doc, cm)
                })
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
internal fun CommentRow(
    vm: AppViewModel,
    doc: OpenDocument,
    cm: CommentInfo,
    reply: Boolean,
    lineAbove: Boolean,
    lineBelow: Boolean,
    replyLink: Boolean,
    lastOnPage: Boolean,
    snippet: ImageBitmap?,
    onSnippet: (ImageBitmap) -> Unit,
    onOpen: () -> Unit,
    onReply: () -> Unit,
    onMenu: () -> Unit,
    /** In the comment panel: no action line over text, and the ⋮ menu drawn by the caller. */
    panel: Boolean = false,
    menu: @Composable () -> Unit = {},
) {
    val c = pdfColors
    val density = LocalDensity.current
    val text = cm.contents?.takeIf { it.isNotBlank() }
    val hasSnippet = !reply && cm.subtype in SNIPPETS && cm.rect.size == 4
    // markups with text show their picture instead of the line; anything without text shows the line
    val actionLine = text == null || (!panel && ((!reply && !hasSnippet) || cm.subtype == "Caret"))
    Box(
        Modifier.fillMaxWidth().clickable(onClick = onOpen).drawBehind {
            // the thread's line runs through the avatars, 2 dp clear of each
            val x = 32.dp.toPx()
            val w = 2.dp.toPx()
            val avatarTop = 8.dp.toPx()
            val avatarBottom = avatarTop + 40.dp.toPx()
            if (lineAbove) drawRect(c.divider, Offset(x - w / 2, 0f), Size(w, avatarTop - 2.dp.toPx()))
            if (lineBelow) drawRect(c.divider, Offset(x - w / 2, avatarBottom + 2.dp.toPx()), Size(w, size.height - avatarBottom - 2.dp.toPx()))
        },
    ) {
        Column(Modifier.fillMaxWidth().padding(start = 56.dp, end = 16.dp)) {
            Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.Top) {
                Column(Modifier.weight(1f)) {
                    Text(
                        cm.author?.takeIf { it.isNotBlank() } ?: stringResource(R.string.comments_guest), color = c.label, fontSize = 16.sp, fontWeight = FontWeight.SemiBold,
                        maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 200.dp),
                    )
                    commentDateLabel(cm.modified, stringResource(R.string.common_today), stringResource(R.string.common_yesterday))?.let { Text(it, color = c.textTertiary, fontSize = 16.sp) }
                }
                Box {
                    BarIcon(LucideIcons.EllipsisVertical, stringResource(R.string.comments_options), onMenu, size = 40.dp)
                    menu()
                }
            }
            if (hasSnippet) {
                Spacer(Modifier.height(12.dp))
                BoxWithConstraints(Modifier.fillMaxWidth().height(16.dp)) {
                    val widthPx = with(density) { maxWidth.toPx() }
                    LaunchedEffect(cm.page, cm.index, doc.info.generation) {
                        if (snippet != null) return@LaunchedEffect
                        val eng = vm.engine ?: return@LaunchedEffect
                        val r = cm.rect
                        val h = (r[3] - r[1]).coerceAtLeast(1f)
                        // rendered so the mark is 30 dp tall, then shown 16 dp tall
                        val scale = (with(density) { 30.dp.toPx() } / h).coerceIn(0.2f, 12f)
                        val pageW = doc.info.pages.getOrNull(cm.page.toInt())?.width ?: return@LaunchedEffect
                        val x = (r[0] * scale).toInt().coerceAtLeast(0)
                        val w = minOf((widthPx * 30f / 16f).toInt(), (pageW * scale).toInt() - x).coerceAtLeast(1)
                        val tile = TileRect(x.toUInt(), (r[1] * scale).toInt().coerceAtLeast(0).toUInt(), w.toUInt(), (h * scale).toInt().coerceAtLeast(1).toUInt())
                        val img = runCatching {
                            withContext(eng.thumbnails) { eng.session.renderPage(doc.id, cm.page, scale, tile).toBitmap().asImageBitmap() }
                        }.getOrNull()
                        if (img != null) onSnippet(img)
                    }
                    if (snippet != null) {
                        Image(snippet, null, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.FillHeight, alignment = Alignment.CenterStart)
                    } else {
                        Box(Modifier.fillMaxSize().background(Color.White))
                    }
                }
            }
            if (actionLine) {
                Text(
                    stringResource(commentActionLabel(cm)), color = c.icon, fontSize = 14.5.sp, fontStyle = FontStyle.Italic,
                    modifier = Modifier.padding(top = if (hasSnippet) 8.dp else 12.dp),
                )
            }
            if (text != null) {
                Text(text, color = c.label, fontSize = 16.sp, lineHeight = 22.5.sp, modifier = Modifier.padding(top = 16.dp))
            }
            if (panel && cm.subtype == "Sound") VoiceCommentPlayer(vm, doc, cm)
            Spacer(Modifier.height(if (replyLink) 16.dp else if (lastOnPage) 12.dp else 8.dp))
            if (replyLink) {
                Text(
                    stringResource(R.string.comments_reply), color = c.toolAccent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.offset(x = (-8).dp).heightIn(min = 40.dp).clip(RoundedCornerShape(4.dp))
                        .clickable(role = Role.Button, onClick = onReply).padding(horizontal = 8.dp, vertical = 10.dp),
                )
                Spacer(Modifier.height(if (lastOnPage) 14.dp else 6.dp))
            }
        }
        Avatar(Modifier.padding(start = 16.dp, top = 12.dp))
    }
}

/** The guest avatar: a lilac disc, its lower right a shade darker. */
@Composable
private fun Avatar(modifier: Modifier) {
    val description = stringResource(R.string.comments_avatar)
    Canvas(modifier.size(32.dp).semantics { contentDescription = description }) {
        val base = Color(0xFF9B80FF)
        drawCircle(base)
        val shade = Path().apply {
            moveTo(size.width, 0f)
            lineTo(size.width, size.height)
            lineTo(0f, size.height)
            close()
        }
        clipPath(Path().apply { addOval(androidx.compose.ui.geometry.Rect(0f, 0f, size.width, size.height)) }) {
            drawPath(shade, Color(base.red * 0.75f, base.green * 0.75f, base.blue * 0.75f))
        }
    }
}

/** No comments yet: a heading and a line on how to add one. */
@Composable
private fun EmptyComments() {
    val c = pdfColors
    Column(
        Modifier.fillMaxSize().padding(horizontal = 64.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(LucideIcons.MessagesSquare, null, tint = c.textSecondary, modifier = Modifier.size(64.dp))
        Spacer(Modifier.height(24.dp))
        Text(stringResource(R.string.comments_empty_title), color = c.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(8.dp))
        Text(stringResource(R.string.comments_empty_body), color = c.textSecondary, fontSize = 17.sp, textAlign = TextAlign.Center)
        Spacer(Modifier.height(80.dp))
    }
}
