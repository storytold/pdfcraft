package io.github.dikshant363.pdfcraft.ui.viewer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.combinedClickable
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.core.AttachmentInfo
import io.github.dikshant363.pdfcraft.core.CommentInfo
import io.github.dikshant363.pdfcraft.core.OutlineEntry
import io.github.dikshant363.pdfcraft.plural
import io.github.dikshant363.pdfcraft.text
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.OpenDocument
import io.github.dikshant363.pdfcraft.ui.PdfSheet
import io.github.dikshant363.pdfcraft.ui.PdfSwitch
import io.github.dikshant363.pdfcraft.ui.SheetBackHeader
import io.github.dikshant363.pdfcraft.ui.GroupDivider
import io.github.dikshant363.pdfcraft.ui.SheetDivider
import io.github.dikshant363.pdfcraft.ui.SheetRow
import io.github.dikshant363.pdfcraft.ui.SheetTitle
import io.github.dikshant363.pdfcraft.ui.pdfColors
import java.text.DateFormat
import java.util.Date

/**
 * More: a header (name, "PDF · modified", Print, Star) over the viewer's commands in the
 * reference's order. Drill-in rows (with a chevron) open their own sheet.
 */
@Composable
fun MoreSheet(vm: AppViewModel, doc: OpenDocument) {
    val c = pdfColors
    val ctl = doc.controller
    fun close() {
        ctl.sheet = ViewerSheet.None
    }
    val bookmarked = vm.bookmarks.isBookmarked(doc, doc.viewer.currentPage)
    val starred = vm.isStarred(doc)
    // like the reference, the sheet opens part way and rises to full height once its list scrolls
    val scroll = rememberScrollState()
    var expanded by remember { mutableStateOf(false) }
    LaunchedEffect(scroll.value > 0) { if (scroll.value > 0) expanded = true }
    val fraction by androidx.compose.animation.core.animateFloatAsState(if (expanded) 1f else 0.69f, label = "sheet")
    val density = androidx.compose.ui.platform.LocalDensity.current
    val top = with(density) {
        maxOf(
            androidx.compose.foundation.layout.WindowInsets.statusBars.getTop(density),
            androidx.compose.foundation.layout.WindowInsets.displayCutout.getTop(density),
        ).toDp()
    } + TopBarHeight
    PdfSheet(onDismiss = ::close, maxHeightFraction = fraction, topReserve = top) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 4.dp, bottom = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(doc.title, color = c.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.height(4.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(LucideIcons.Smartphone, stringResource(R.string.sheet_stored_on_device), tint = c.icon, modifier = Modifier.size(16.dp))
                    Spacer(Modifier.width(6.dp))
                    Text(
                        "PDF • ${modifiedLabel(doc.file.lastModified(), stringResource(R.string.common_yesterday))}" + if (!doc.writable) " • " + stringResource(R.string.sheet_read_only) else "",
                        color = c.text, fontSize = 13.sp,
                    )
                }
            }
            BarIcon(LucideIcons.Printer, stringResource(R.string.sheet_print), { close(); vm.print(doc) }, size = 40.dp)
            BarIcon(LucideIcons.Star, stringResource(if (starred) R.string.sheet_remove_star else R.string.sheet_add_star), { vm.toggleStar(doc) },
                tint = if (starred) Color(0xFFF5B800) else c.icon, size = 40.dp)
        }
        SheetDivider()
        Column(Modifier.verticalScroll(scroll).padding(top = 5.dp)) {
            SheetRow(LucideIcons.Glasses, stringResource(R.string.sheet_view_settings), { ctl.sheet = ViewerSheet.ViewSettings }, chevron = true)
            SheetRow(LucideIcons.LayoutGrid, stringResource(R.string.sheet_pages), { close(); ctl.pagesOpen = true }, chevron = true)
            if (doc.info.attachments > 0u) SheetRow(LucideIcons.Paperclip, stringResource(R.string.sheet_attachments), { ctl.sheet = ViewerSheet.Attachments }, chevron = true)
            SheetRow(LucideIcons.Bookmark, stringResource(R.string.sheet_bookmarks_contents), { ctl.sheet = ViewerSheet.Bookmarks }, chevron = true)
            SheetRow(LucideIcons.MessagesSquare, stringResource(R.string.sheet_comment_list), { ctl.sheet = ViewerSheet.Comments }, chevron = true)
            SheetRow(LucideIcons.ShieldCheck, stringResource(R.string.sign_signatures), { ctl.sheet = ViewerSheet.Signatures }, chevron = true)
            SheetRow(
                if (bookmarked) LucideIcons.BookmarkMinus else LucideIcons.BookmarkPlus,
                stringResource(if (bookmarked) R.string.sheet_remove_bookmark else R.string.sheet_add_bookmark),
                { close(); vm.toggleBookmark(doc) },
            )
            SheetRow(LucideIcons.Volume2, stringResource(R.string.common_tool_read_aloud), { close(); vm.startReadAloud(doc) })
            SheetRow(LucideIcons.FilePenLine, stringResource(R.string.common_tool_edit_pdf), { close(); vm.enterTool(doc, ViewerTool.Edit) })
            GroupDivider()
            SheetRow(LucideIcons.FileOutput, stringResource(R.string.sheet_export_as), { close(); vm.openExport(doc) })
            SheetRow(LucideIcons.FileArchive, stringResource(R.string.common_tool_compress), { close(); vm.openCompress(doc) })
            SheetRow(LucideIcons.Combine, stringResource(R.string.common_tool_combine), { close(); vm.openCombine(doc) })
            SheetRow(LucideIcons.FileLock2, stringResource(R.string.common_tool_set_password), { close(); vm.openProtect(doc) })
            SheetRow(LucideIcons.FilePlus2, stringResource(R.string.common_tool_create_pdf), { close(); vm.openCreate() })
            if (doc.info.undo != null || doc.info.redo != null) {
                GroupDivider()
                doc.info.undo?.let { SheetRow(LucideIcons.Undo2, stringResource(R.string.sheet_undo_action, it), { close(); vm.undo(doc) }) }
                doc.info.redo?.let { SheetRow(LucideIcons.Redo2, stringResource(R.string.sheet_redo_action, it), { close(); vm.redo(doc) }) }
            }
            GroupDivider()
            SheetRow(LucideIcons.Copy, stringResource(R.string.common_save_copy), { close(); vm.saveCopyAs(doc) })
            Spacer(Modifier.height(8.dp))
        }
    }
}

/** When the file was modified: the time today, [yesterday] ("Yesterday"), else the date. */
internal fun modifiedLabel(t: Long, yesterday: String, now: Long = System.currentTimeMillis()): String {
    fun day(ms: Long) = java.util.Calendar.getInstance().apply { timeInMillis = ms }.let { it.get(java.util.Calendar.YEAR) * 400 + it.get(java.util.Calendar.DAY_OF_YEAR) }
    val d = Date(t)
    val today = day(now)
    val dayBefore = day(now - 24L * 3600 * 1000)
    return when (day(t)) {
        today -> DateFormat.getTimeInstance(DateFormat.SHORT).format(d)
        dayBefore -> yesterday
        else -> DateFormat.getDateInstance(DateFormat.MEDIUM).format(d)
    }
}

/** View settings: Continuous, Single page, Reading Mode, then the Night Mode switch. */
@Composable
fun ViewSettingsSheet(vm: AppViewModel, doc: OpenDocument) {
    val ctl = doc.controller
    PdfSheet(onDismiss = { ctl.sheet = ViewerSheet.None }) {
        SheetBackHeader(stringResource(R.string.sheet_view_settings), onBack = { ctl.sheet = ViewerSheet.More })
        SheetRow(LucideIcons.GalleryVerticalEnd, stringResource(R.string.sheet_continuous), {
            vm.setPageMode(doc, PageMode.Continuous)
            ctl.sheet = ViewerSheet.None
        }, checked = doc.viewer.mode == PageMode.Continuous && !vm.readingMode)
        SheetRow(LucideIcons.File, stringResource(R.string.sheet_single_page), {
            vm.setPageMode(doc, PageMode.SinglePage)
            ctl.sheet = ViewerSheet.None
        }, checked = doc.viewer.mode == PageMode.SinglePage && !vm.readingMode)
        SheetRow(LucideIcons.BookOpenText, stringResource(R.string.sheet_reading_mode), {
            ctl.sheet = ViewerSheet.None
            vm.openReadingMode(doc)
        }, checked = vm.readingMode)
        GroupDivider()
        SheetRow(LucideIcons.Moon, stringResource(R.string.sheet_night_mode), { vm.setNightMode(doc, !ctl.nightMode); ctl.sheet = ViewerSheet.None }) {
            PdfSwitch(ctl.nightMode, { vm.setNightMode(doc, it); ctl.sheet = ViewerSheet.None })
        }
        Spacer(Modifier.height(24.dp))
    }
}

/** Bookmarks & Table of Contents: the outline, one level at a time, and the reader's own bookmarks. */
@Composable
fun BookmarksSheet(vm: AppViewModel, doc: OpenDocument) {
    val c = pdfColors
    val ctl = doc.controller
    var outline by remember { mutableStateOf<List<OutlineEntry>?>(null) }
    LaunchedEffect(doc.info.generation) { outline = vm.outline(doc) }
    val hasOutline = doc.info.hasOutline
    var tab by remember { mutableStateOf(if (hasOutline) 0 else 1) }
    // the outline level shown: the index of its parent entry (-1: the top level)
    var parent by remember { mutableStateOf(-1) }
    var renaming by remember { mutableStateOf<Int?>(null) }
    PdfSheet(onDismiss = { ctl.sheet = ViewerSheet.None }, maxHeightFraction = 0.69f, fill = true) {
        SheetBackHeader(stringResource(R.string.sheet_bookmarks_contents), onBack = { ctl.sheet = ViewerSheet.More })
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp)) {
            listOf(stringResource(R.string.sheet_tab_contents), stringResource(R.string.sheet_tab_bookmarks)).forEachIndexed { i, label ->
                Column(
                    Modifier.weight(1f).clickable(role = Role.Tab) { tab = i }.padding(top = 8.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(label, color = if (tab == i) c.text else c.textSecondary, fontSize = 15.sp, fontWeight = if (tab == i) FontWeight.SemiBold else FontWeight.Normal)
                    Spacer(Modifier.height(8.dp))
                    Box(Modifier.fillMaxWidth().height(2.dp).background(if (tab == i) c.accent else Color.Transparent))
                }
            }
        }
        SheetDivider(inset = 0.dp)
        if (tab == 0) {
            val all = outline
            when {
                all == null -> Text("…", color = c.textSecondary, modifier = Modifier.padding(20.dp))
                all.isEmpty() -> Text(stringResource(R.string.sheet_no_contents), color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(20.dp))
                else -> {
                    val level = if (parent < 0) 0u else all[parent].level + 1u
                    val start = parent + 1
                    // the entries of this level under `parent`: until the level rises back above it
                    val entries = buildList {
                        var i = start
                        while (i < all.size) {
                            val e = all[i]
                            if (e.level < level) break
                            if (e.level == level) add(i)
                            i++
                        }
                    }
                    LazyColumn(Modifier.weight(1f)) {
                        if (parent >= 0) {
                            item {
                                Row(Modifier.fillMaxWidth().clickable { parent = parentOf(all, parent) }.padding(horizontal = 12.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                                    Icon(LucideIcons.ArrowLeft, stringResource(R.string.sheet_up_level), tint = c.icon, modifier = Modifier.size(22.dp))
                                    Spacer(Modifier.width(16.dp))
                                    Text(all[parent].title, color = c.text, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                }
                            }
                        }
                        items(entries) { i ->
                            val e = all[i]
                            Row(
                                Modifier.fillMaxWidth().clickable {
                                    e.page?.let { p ->
                                        ctl.rememberJump(doc.viewer.currentPage)
                                        doc.viewer.goToPage(p.toInt())
                                        ctl.sheet = ViewerSheet.None
                                    }
                                }.padding(start = 20.dp, end = 4.dp).heightIn(min = 48.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(e.title, color = c.text, fontSize = 16.sp, modifier = Modifier.weight(1f).padding(vertical = 10.dp), maxLines = 3, overflow = TextOverflow.Ellipsis)
                                if (e.children > 0u) BarIcon(LucideIcons.ChevronRight, stringResource(R.string.sheet_expand_entry, e.title), { parent = i })
                            }
                        }
                    }
                }
            }
        } else {
            val list = vm.bookmarks.list(doc)
            if (list.isEmpty()) {
                Column(Modifier.fillMaxWidth().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(LucideIcons.Bookmark, null, tint = c.textSecondary, modifier = Modifier.size(40.dp))
                    Spacer(Modifier.height(12.dp))
                    Text(stringResource(R.string.sheet_bookmarks_empty), color = c.text, fontSize = 15.sp)
                    Spacer(Modifier.height(16.dp))
                    Text(stringResource(R.string.sheet_add_a_bookmark), color = c.accent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.clickable { vm.toggleBookmark(doc) }.padding(8.dp))
                }
            } else {
                LazyColumn(Modifier.weight(1f)) {
                    items(list.size) { i ->
                        val b = list[i]
                        UserBookmarkRow(
                            name = b.name,
                            page = b.page,
                            renaming = renaming == i,
                            onOpen = {
                                ctl.rememberJump(doc.viewer.currentPage)
                                doc.viewer.goToPage(b.page)
                                ctl.sheet = ViewerSheet.None
                            },
                            onRename = { renaming = i },
                            onRenamed = { name ->
                                renaming = null
                                if (name.isBlank()) vm.tell(text(R.string.sheet_bookmark_name_invalid)) else vm.bookmarks.rename(doc, b, name.trim())
                            },
                            onDelete = { vm.bookmarks.remove(doc, b); vm.tell(text(R.string.sheet_bookmark_removed, b.name)) },
                            onDeleteAll = { vm.confirmDeleteAllBookmarks = doc },
                        )
                    }
                }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

private fun parentOf(all: List<OutlineEntry>, i: Int): Int {
    val level = all[i].level
    if (level == 0u) return -1
    var k = i - 1
    while (k >= 0 && all[k].level >= level) k--
    return k
}

@Composable
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
private fun UserBookmarkRow(name: String, page: Int, renaming: Boolean, onOpen: () -> Unit, onRename: () -> Unit, onRenamed: (String) -> Unit, onDelete: () -> Unit, onDeleteAll: () -> Unit) {
    val c = pdfColors
    var menu by remember { mutableStateOf(false) }
    var text by remember(renaming) { mutableStateOf(name) }
    // a long press opens Rename / Delete / Delete all, as the reference's list does
    Row(
        Modifier.fillMaxWidth().combinedClickable(enabled = !renaming, onClick = onOpen, onLongClick = { menu = true })
            .padding(start = 20.dp, end = 4.dp).heightIn(min = 52.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(LucideIcons.Bookmark, null, tint = c.icon, modifier = Modifier.size(20.dp))
        Spacer(Modifier.width(16.dp))
        if (renaming) {
            androidx.compose.material3.OutlinedTextField(
                value = text, onValueChange = { text = it }, singleLine = true, modifier = Modifier.weight(1f),
                keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = androidx.compose.ui.text.input.ImeAction.Done),
                keyboardActions = androidx.compose.foundation.text.KeyboardActions(onDone = { onRenamed(text) }),
            )
        } else {
            Column(Modifier.weight(1f)) {
                Text(name, color = c.text, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(stringResource(R.string.sheet_page, page + 1), color = c.textSecondary, fontSize = 13.sp)
            }
        }
        Box {
            androidx.compose.material3.DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = c.menu) {
                androidx.compose.material3.DropdownMenuItem(text = { Text(stringResource(R.string.common_rename), color = c.text) }, onClick = { menu = false; onRename() })
                androidx.compose.material3.DropdownMenuItem(text = { Text(stringResource(R.string.common_delete), color = c.text) }, onClick = { menu = false; onDelete() })
                androidx.compose.material3.DropdownMenuItem(text = { Text(stringResource(R.string.sheet_delete_all), color = c.text) }, onClick = { menu = false; onDeleteAll() })
            }
        }
    }
}

/** Attachments: the embedded files; a PDF opens in the viewer, anything else in another app. */
@Composable
fun AttachmentsSheet(vm: AppViewModel, doc: OpenDocument) {
    val c = pdfColors
    val ctl = doc.controller
    var list by remember { mutableStateOf<List<AttachmentInfo>>(emptyList()) }
    LaunchedEffect(doc.info.generation) { list = vm.attachments(doc) }
    // the document's own files, as the reference lists them (a comment's file is opened from the comment)
    val files = list.filter { !it.onPage }
    PdfSheet(onDismiss = { ctl.sheet = ViewerSheet.None }, maxHeightFraction = 0.7f) {
        // the reference sets this title up level with the handle and its back arrow below it
        Box(Modifier.fillMaxWidth().height(44.dp)) {
            Text(
                stringResource(R.string.sheet_attachments), color = c.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.align(Alignment.TopStart).offset(x = 92.dp, y = (-12).dp),
            )
            BarIcon(LucideIcons.ChevronLeft, stringResource(R.string.common_back), { ctl.sheet = ViewerSheet.More }, modifier = Modifier.align(Alignment.TopStart).offset(x = 3.dp, y = (-5).dp))
        }
        LazyColumn {
            items(files) { a ->
                Row(
                    Modifier.fillMaxWidth().clickable { ctl.sheet = ViewerSheet.None; vm.openAttachment(doc, a.name) }.padding(start = 24.dp, end = 20.dp, top = 16.dp, bottom = 16.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(LucideIcons.FileText, null, tint = c.icon, modifier = Modifier.size(24.dp))
                    Spacer(Modifier.width(14.dp))
                    Text(a.name, color = c.text, fontSize = 16.sp, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

fun formatSize(bytes: Long): String = when {
    bytes < 1024 -> plural(R.plurals.sheet_size_bytes, bytes.toInt(), bytes)
    bytes < 1024 * 1024 -> text(R.string.sheet_size_kb, bytes / 1024.0)
    else -> text(R.string.sheet_size_mb, bytes / 1024.0 / 1024.0)
}

/** `D:20261011143000+02'00'` (or plain text) as a short local date. */
fun pdfDateLabel(s: String?): String? {
    if (s == null) return null
    val t = pdfDateMillis(s) ?: return s.takeIf { it.isNotBlank() }
    return DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(t))
}

/**
 * The moment a PDF date names (`D:20261011171900Z`, `D:20261011171900-06'00'`, the engine's
 * `2026-10-11 17:19`, or a certificate's `2031.10.19 13:59:58 UTC`).
 */
fun pdfDateMillis(s: String?): Long? {
    if (s == null) return null
    val m = Regex("""(\d{4})[-/.]?(\d{2})?[-/.]?(\d{2})?[ T]?(\d{2})?:?(\d{2})?:?(\d{2})? ?(Z|UTC|[+-]\d{2}'?\d{2}'?)?""").find(s) ?: return null
    val g = m.groupValues
    // with a time zone the moment is exact and shows in the phone's local time; without one, as written
    val tz = g[7]
    val zone = when {
        tz.isEmpty() -> java.util.TimeZone.getDefault()
        tz == "Z" || tz == "UTC" -> java.util.TimeZone.getTimeZone("UTC")
        else -> java.util.TimeZone.getTimeZone("GMT" + tz.substring(0, 3) + ":" + tz.filter { it.isDigit() }.drop(2).padEnd(2, '0').take(2))
    }
    val cal = java.util.Calendar.getInstance(zone)
    cal.clear()
    cal.set(g[1].toInt(), (g[2].ifEmpty { "1" }.toInt() - 1), g[3].ifEmpty { "1" }.toInt(), g[4].ifEmpty { "0" }.toInt(), g[5].ifEmpty { "0" }.toInt(), g[6].ifEmpty { "0" }.toInt())
    return cal.timeInMillis
}

/** A tool tile of More tools: a tinted 56 dp rounded square with the tool's icon, its name under it. */
@Composable
private fun ToolTile(icon: ImageVector, label: String, tint: Color, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = pdfColors
    Column(
        modifier.clickable(role = Role.Button, onClick = onClick).padding(vertical = 6.dp).semantics { contentDescription = label },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(Modifier.size(56.dp).clip(RoundedCornerShape(6.dp)).background(tileBackground(tint, c.dark)), contentAlignment = Alignment.Center) {
            Icon(icon, null, tint = tint, modifier = Modifier.size(30.dp))
        }
        Spacer(Modifier.height(11.dp))
        Text(label, color = c.text, fontSize = 11.sp, textAlign = TextAlign.Center, maxLines = 2, lineHeight = 14.sp, modifier = Modifier.padding(horizontal = 2.dp))
    }
}

internal fun tileBackground(tint: Color, dark: Boolean): Color =
    if (dark) Color(tint.red * 0.22f + 0.07f, tint.green * 0.22f + 0.07f, tint.blue * 0.22f + 0.09f) else Color(tint.red * 0.12f + 0.88f, tint.green * 0.12f + 0.88f, tint.blue * 0.12f + 0.88f)

/** The tool colours of More tools and the Tools tab (each tool its own hue). */
object ToolHues {
    val combine = Color(0xFF8F8CF5)
    val compress = Color(0xFF3CC8C2)
    val protect = Color(0xFF8F8CF5)
    val organize = Color(0xFF9FD848)
    val crop = Color(0xFF6E9BF2)
    val ocr = Color(0xFF5BCB6B)
    val create = Color(0xFFF2615C)
    val text = Color(0xFFF2A93B)
    val scan = Color(0xFF3CC8C2)
    val read = Color(0xFF6E9BF2)
    val edit = Color(0xFFEC5BA6)
    val export = Color(0xFFF2615C)
    val sign = Color(0xFF9B7CF2)
    val comment = Color(0xFFF2A93B)
    val redact = Color(0xFFF2615C)
}

/** More tools: the tools that aren't on the bar, as a grid of tiles, and Customize toolbar. */
@Composable
fun MoreToolsSheet(vm: AppViewModel, doc: OpenDocument) {
    val ctl = doc.controller
    var customizing by remember { mutableStateOf(false) }
    PdfSheet(onDismiss = { if (customizing) customizing = false else ctl.sheet = ViewerSheet.None }) {
        if (customizing) {
            CustomizeToolbar(vm) { customizing = false }
            return@PdfSheet
        }
        SheetTitle(stringResource(R.string.common_tool_more_tools))
        val slots = toolbarSlots(androidx.compose.ui.platform.LocalConfiguration.current.screenWidthDp)
        val prefs = vm.quickToolbar
        val tools = prefs.bar.drop(slots) + prefs.grid
        // six to a row in 85 dp columns, as the reference lays them out
        androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth().padding(top = 5.dp)) {
            val cell = minOf(85.dp, (maxWidth - 2.dp) / 6)
            Column(Modifier.padding(start = 1.dp)) {
                tools.chunked(6).forEach { row ->
                    Row(verticalAlignment = Alignment.Top) {
                        row.forEach { t ->
                            ToolTile(t.icon, stringResource(t.title), t.hue, Modifier.width(cell)) {
                                ctl.sheet = ViewerSheet.None
                                vm.runQuickTool(doc, t)
                            }
                        }
                    }
                    Spacer(Modifier.height(17.dp))
                }
            }
        }
        // Customize toolbar: an outlined pill across the sheet
        Box(
            Modifier.padding(horizontal = 36.dp).fillMaxWidth().height(41.dp).clip(RoundedCornerShape(20.5.dp))
                .border(2.dp, pdfColors.divider, RoundedCornerShape(20.5.dp))
                .clickable(role = Role.Button) { customizing = true },
            contentAlignment = Alignment.Center,
        ) {
            Text(stringResource(R.string.sheet_customize_toolbar), color = pdfColors.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        }
        Spacer(Modifier.height(16.dp))
    }
}
