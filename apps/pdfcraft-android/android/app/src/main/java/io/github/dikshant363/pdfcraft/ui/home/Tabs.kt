package io.github.dikshant363.pdfcraft.ui.home

import io.github.dikshant363.pdfcraft.ui.safeTopPadding
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.border
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.annotation.StringRes
import io.github.dikshant363.pdfcraft.R
import io.github.dikshant363.pdfcraft.ui.AppViewModel
import io.github.dikshant363.pdfcraft.ui.BarIcon
import io.github.dikshant363.pdfcraft.ui.Inter
import io.github.dikshant363.pdfcraft.ui.LucideIcons
import io.github.dikshant363.pdfcraft.ui.PdfSwitch
import io.github.dikshant363.pdfcraft.ui.SheetDivider
import io.github.dikshant363.pdfcraft.ui.SheetRow
import io.github.dikshant363.pdfcraft.ui.pdfColors
import io.github.dikshant363.pdfcraft.ui.viewer.ToolHues

/** Files: the places files come from; On this device lists every PDF on the phone by folder. */
@Composable
fun FilesTab(vm: AppViewModel, onBrowse: () -> Unit) {
    val c = pdfColors
    if (vm.filesShowLocal) {
        OnThisDevice(vm)
        return
    }
    Column(Modifier.fillMaxSize().safeTopPadding()) {
        // a large title, as the expanded bar of the reference shows it
        Box(Modifier.fillMaxWidth().height(128.dp), contentAlignment = Alignment.BottomStart) {
            Text(stringResource(R.string.home_tab_files), color = c.label, fontSize = 24.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(start = 16.dp, bottom = 40.dp))
        }
        LocationRow(LucideIcons.Smartphone, stringResource(R.string.home_on_this_device)) { vm.filesShowLocal = true }
        LocationRow(LucideIcons.Image, stringResource(R.string.home_photos)) { vm.pickPhotosToPdf() }
        LocationRow(LucideIcons.FolderOpen, stringResource(R.string.home_look_in_other_folders), onBrowse)
    }
}

@Composable
private fun LocationRow(icon: ImageVector, label: String, onClick: () -> Unit) {
    val c = pdfColors
    Row(Modifier.fillMaxWidth().heightIn(min = 62.dp).clickable(onClick = onClick).padding(start = 16.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, tint = c.icon, modifier = Modifier.size(22.dp))
        Spacer(Modifier.width(16.dp))
        Text(label, color = c.text, fontSize = 17.sp, modifier = Modifier.weight(1f))
    }
}

/** On this device: every PDF found on the phone, newest first, under its folder's name. */
@Composable
private fun OnThisDevice(vm: AppViewModel) {
    val c = pdfColors
    BackHandler { vm.filesShowLocal = false }
    LaunchedEffect(Unit) { vm.scanLocal() }
    var sort by remember { mutableStateOf(0) }
    var menu by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().safeTopPadding()) {
        Row(Modifier.fillMaxWidth().height(56.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(56.dp), contentAlignment = Alignment.Center) { BarIcon(LucideIcons.ArrowLeft, stringResource(R.string.common_back), { vm.filesShowLocal = false }) }
            Text(stringResource(R.string.home_on_this_device), color = c.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            BarIcon(LucideIcons.RotateCw, stringResource(R.string.home_refresh), { vm.scanLocal(force = true) })
            Box {
                BarIcon(LucideIcons.EllipsisVertical, stringResource(R.string.common_more_options), { menu = true })
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = c.surface) {
                    DropdownMenuItem(text = { Text(stringResource(R.string.home_sort_by_name)) }, onClick = { sort = 1; menu = false }, trailingIcon = { if (sort == 1) Icon(LucideIcons.Check, null, tint = c.accent) })
                    DropdownMenuItem(text = { Text(stringResource(R.string.home_sort_by_date)) }, onClick = { sort = 0; menu = false }, trailingIcon = { if (sort == 0) Icon(LucideIcons.Check, null, tint = c.accent) })
                    HorizontalDivider(color = c.divider)
                    DropdownMenuItem(text = { Text(stringResource(R.string.home_add_folder)) }, onClick = { menu = false; vm.requestFileAccess() }, leadingIcon = { Icon(LucideIcons.FolderPlus, null, tint = c.icon) })
                    // the folders listed here, each with a way to stop listing it
                    for ((tree, name) in vm.localFolders()) {
                        DropdownMenuItem(text = { Text(stringResource(R.string.home_stop_listing, name)) }, onClick = { menu = false; vm.removeLocalFolder(tree) }, leadingIcon = { Icon(LucideIcons.Folder, null, tint = c.icon) })
                    }
                }
            }
        }
        val rows = vm.localRows().let { if (sort == 1) it.sortedBy { r -> r.name.lowercase() } else it }
        // what PdfCraft saved itself shows before any folder is added
        if (!vm.hasFileAccess && rows.isEmpty()) {
            EmptyState(LucideIcons.Smartphone, stringResource(R.string.home_device_files_title), stringResource(R.string.home_device_files_body_long), stringResource(R.string.home_add_folder)) { vm.requestFileAccess() }
            return@Column
        }
        if (rows.isEmpty()) {
            if (vm.scanning) EmptyState(LucideIcons.FileText, stringResource(R.string.home_looking_for_pdfs), "")
            else EmptyState(LucideIcons.FileText, stringResource(R.string.home_no_pdfs_title), stringResource(R.string.home_no_pdfs_body), stringResource(R.string.home_add_folder)) { vm.requestFileAccess() }
            return@Column
        }
        LazyColumn(Modifier.weight(1f)) {
            val groups = rows.groupBy { it.location }
            for ((folder, list) in groups) {
                item(key = "h-$folder") {
                    val internalStorage = stringResource(R.string.home_internal_storage)
                    Text(folder.uppercase().ifEmpty { internalStorage }, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 4.dp))
                }
                items(list, key = { it.key }) { r -> FileListRow(vm, r) }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
}

/** A tool of the Tools tab: its tile, name and one line about it ([name] is its key, [label] what is shown). */
private class HubTool(val icon: ImageVector, val name: String, @StringRes val label: Int, @StringRes val about: Int, val hue: Color, val categories: Set<String>, val go: () -> Unit)

/** Where a category lists its tools in an order of its own (the rest keep the All order). */
private val CategoryOrder = mapOf(
    "Edit" to listOf("Edit PDF", "Crop pages", "Organize pages", "Combine files", "Set password"),
    "Convert" to listOf("Export PDF", "Compress PDF"),
)

/** Tools: every tool, by category (All, Read, Create, Edit, Convert), as outlined cards. */
@Composable
fun ToolsTab(vm: AppViewModel) {
    val c = pdfColors
    val categories = listOf(
        "All" to R.string.home_category_all, "Read" to R.string.home_category_read, "Create" to R.string.home_category_create,
        "Edit" to R.string.home_category_edit, "Convert" to R.string.home_category_convert,
    )
    var cat by remember { mutableStateOf("All") }
    val tools = remember {
        listOf(
            HubTool(LucideIcons.Signature, "Fill & Sign", R.string.common_tool_fill_sign, R.string.home_about_fill_sign, ToolHues.sign, emptySet()) { vm.toolForFile(FileTool.FillSign) },
            HubTool(LucideIcons.MessageSquareText, "Add comment", R.string.home_tool_add_comment, R.string.home_about_comment, ToolHues.comment, emptySet()) { vm.toolForFile(FileTool.Comment) },
            HubTool(LucideIcons.Volume2, "Read aloud", R.string.common_tool_read_aloud, R.string.home_about_read_aloud, ToolHues.read, setOf("Read")) { vm.toolForFile(FileTool.ReadAloud) },
            HubTool(LucideIcons.FilePlus2, "Create PDF", R.string.common_tool_create_pdf, R.string.home_about_create, ToolHues.create, setOf("Create")) { vm.openCreate() },
            HubTool(LucideIcons.Camera, "New scan", R.string.home_new_scan, R.string.home_about_scan, ToolHues.scan, setOf("Create")) { vm.startScan() },
            HubTool(LucideIcons.Combine, "Combine files", R.string.common_tool_combine, R.string.home_about_combine, ToolHues.combine, setOf("Create", "Edit")) { vm.pickFiles(io.github.dikshant363.pdfcraft.ui.FilePickKind.Combine) },
            HubTool(LucideIcons.LayoutGrid, "Organize pages", R.string.common_tool_organize, R.string.home_about_organize, ToolHues.organize, setOf("Edit")) { vm.toolForFile(FileTool.Organize) },
            HubTool(LucideIcons.Crop, "Crop pages", R.string.common_tool_crop, R.string.home_about_crop, ToolHues.crop, setOf("Edit")) { vm.toolForFile(FileTool.Crop) },
            HubTool(LucideIcons.FilePenLine, "Edit PDF", R.string.common_tool_edit_pdf, R.string.home_about_edit, ToolHues.edit, setOf("Edit")) { vm.toolForFile(FileTool.Edit) },
            HubTool(LucideIcons.FileArchive, "Compress PDF", R.string.common_tool_compress, R.string.home_about_compress, ToolHues.compress, setOf("Convert")) { vm.toolForFile(FileTool.Compress) },
            HubTool(LucideIcons.FileOutput, "Export PDF", R.string.common_tool_export, R.string.home_about_export, ToolHues.export, setOf("Convert")) { vm.toolForFile(FileTool.Export) },
            HubTool(LucideIcons.FileLock2, "Set password", R.string.common_tool_set_password, R.string.home_about_protect, ToolHues.protect, setOf("Edit")) { vm.toolForFile(FileTool.Protect) },
        )
    }
    Column(Modifier.fillMaxSize().safeTopPadding()) {
        Text(stringResource(R.string.home_tab_tools), color = c.label, fontSize = 27.sp, lineHeight = 35.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(start = 16.dp, top = 12.dp, bottom = 12.dp))
        // text tabs with a 2 dp underline under the chosen one
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(24.dp)) {
            for ((k, label) in categories) {
                Column(Modifier.clickable(role = Role.Tab) { cat = k }.width(androidx.compose.foundation.layout.IntrinsicSize.Max)) {
                    Text(stringResource(label), color = if (k == cat) c.label else c.textTertiary, fontSize = 17.sp, lineHeight = 25.sp, fontWeight = if (k == cat) FontWeight.SemiBold else FontWeight.Normal)
                    Spacer(Modifier.height(14.dp))
                    Box(Modifier.fillMaxWidth().height(2.dp).background(if (k == cat) c.label else Color.Transparent))
                }
            }
        }
        Spacer(Modifier.height(16.dp))
        LazyVerticalGrid(
            GridCells.Adaptive(140.dp), modifier = Modifier.weight(1f).padding(horizontal = 16.dp),
            horizontalArrangement = Arrangement.spacedBy(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            items(tools.filter { cat == "All" || cat in it.categories }.sortedBy { t -> CategoryOrder[cat]?.indexOf(t.name)?.takeIf { it >= 0 } ?: Int.MAX_VALUE }) { t ->
                val name = stringResource(t.label)
                Column(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.surface)
                        .border(1.dp, c.divider, RoundedCornerShape(12.dp)).clickable(onClick = t.go).padding(16.dp).semantics { contentDescription = name },
                ) {
                    Box(Modifier.size(64.dp).clip(RoundedCornerShape(5.dp)).background(tint(t.hue, c.dark)), contentAlignment = Alignment.Center) {
                        Icon(t.icon, null, tint = t.hue, modifier = Modifier.size(32.dp))
                    }
                    Spacer(Modifier.height(12.dp))
                    Text(name, color = c.label, fontSize = 15.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold)
                    Text(stringResource(t.about), color = c.label, fontSize = 15.sp, lineHeight = 21.sp)
                }
            }
            item { Spacer(Modifier.height(8.dp)) }
        }
    }
}

/** Search: file names on the phone and in Recent, with the reference's sort choices. */
@Composable
fun SearchTab(vm: AppViewModel) {
    val c = pdfColors
    var query by remember { mutableStateOf("") }
    var sort by remember { mutableStateOf(0) }
    var menu by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { vm.scanLocal() }
    val searchHint = stringResource(R.string.home_search_your_files)
    // the orders results can be sorted in, by [sort]
    val sorts = listOf(R.string.home_sort_relevant, R.string.home_sort_name_az, R.string.home_sort_name_za, R.string.home_sort_oldest, R.string.home_sort_newest)
    Column(Modifier.fillMaxSize().safeTopPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp).clip(CircleShape).background(c.surfaceRaised).padding(horizontal = 14.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(LucideIcons.Search, null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
            Spacer(Modifier.width(10.dp))
            BasicTextField(
                value = query, onValueChange = { query = it }, singleLine = true,
                keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = androidx.compose.ui.text.input.ImeAction.Search),
                keyboardActions = androidx.compose.foundation.text.KeyboardActions(onSearch = { vm.addRecentSearch(query) }),
                textStyle = TextStyle(fontFamily = Inter, fontSize = 16.sp, color = c.text), cursorBrush = SolidColor(c.accent),
                modifier = Modifier.weight(1f).semantics { contentDescription = searchHint },
                decorationBox = { inner -> Box { if (query.isEmpty()) Text(searchHint, color = c.textSecondary, fontSize = 16.sp); inner() } },
            )
            if (query.isNotEmpty()) BarIcon(LucideIcons.X, stringResource(R.string.common_clear), { query = "" }, size = 32.dp)
        }
        if (query.isBlank()) {
            val recent = vm.recentSearches
            if (recent.isEmpty()) {
                EmptyState(LucideIcons.Search, searchHint, stringResource(R.string.home_search_empty_body))
                return@Column
            }
            // the last searches, to run again with a tap
            Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.home_recent_searches), color = c.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                Text(stringResource(R.string.common_clear), color = c.toolAccent, fontSize = 15.sp, modifier = Modifier.clickable { vm.clearRecentSearches() }.padding(12.dp))
            }
            LazyColumn(Modifier.weight(1f)) {
                items(recent, key = { it }) { q ->
                    Row(Modifier.fillMaxWidth().height(52.dp).clickable { query = q }.padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(LucideIcons.Clock, null, tint = c.icon, modifier = Modifier.size(22.dp))
                        Spacer(Modifier.width(16.dp))
                        Text(q, color = c.text, fontSize = 16.sp, maxLines = 1, modifier = Modifier.weight(1f))
                        BarIcon(LucideIcons.X, stringResource(R.string.home_remove_search, q), { vm.removeRecentSearch(q) }, size = 40.dp)
                    }
                }
            }
            return@Column
        }
        // a search left in place for a moment counts as one made
        LaunchedEffect(query) {
            kotlinx.coroutines.delay(2000)
            if (vm.searchFiles(query).isNotEmpty()) vm.addRecentSearch(query)
        }
        val results = vm.searchFiles(query).let { list ->
            when (sort) {
                1 -> list.sortedBy { it.name.lowercase() }
                2 -> list.sortedByDescending { it.name.lowercase() }
                3 -> list.sortedBy { it.date }
                4 -> list.sortedByDescending { it.date }
                // most relevant: names starting with the search first, then the newest
                else -> list.sortedWith(compareBy<FileRow> { !it.name.lowercase().startsWith(query.trim().lowercase()) }.thenByDescending { it.date })
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(pluralStringResource(R.plurals.home_search_results, results.size, results.size), color = c.text, fontSize = 15.sp, modifier = Modifier.weight(1f))
            Box {
                Row(Modifier.clickable { menu = true }.padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(sorts[sort]), color = c.label, fontSize = 15.sp)
                    Spacer(Modifier.width(4.dp))
                    Icon(LucideIcons.ArrowUpDown, null, tint = c.icon, modifier = Modifier.size(18.dp))
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = c.surface) {
                    sorts.forEachIndexed { i, s ->
                        DropdownMenuItem(text = { Text(stringResource(s)) }, onClick = { sort = i; menu = false }, trailingIcon = { if (sort == i) Icon(LucideIcons.Check, null, tint = c.accent) })
                    }
                }
            }
        }
        if (results.isEmpty()) {
            EmptyState(LucideIcons.FileSearch, stringResource(R.string.home_no_results), stringResource(R.string.home_no_results_body))
        } else {
            LazyColumn(Modifier.weight(1f)) { items(results, key = { it.key }) { r -> FileListRow(vm, r) } }
        }
    }
}

/** Settings: About, Preferences, Help & Support. */
@Composable
fun SettingsScreen(vm: AppViewModel) {
    val c = pdfColors
    var page by remember { mutableStateOf("root") }
    BackHandler { if (page == "root") vm.settingsOpen = false else page = "root" }
    Column(Modifier.fillMaxSize().background(c.surface).safeTopPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().height(56.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(56.dp), contentAlignment = Alignment.Center) {
                BarIcon(LucideIcons.ArrowLeft, stringResource(R.string.common_back), { if (page == "root") vm.settingsOpen = false else page = "root" })
            }
            Text(stringResource(when (page) { "about" -> R.string.home_about_app; "prefs" -> R.string.home_preferences; "help" -> R.string.home_help_support; else -> R.string.home_settings }), color = c.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        }
        Column(Modifier.verticalScroll(rememberScrollState())) {
            when (page) {
                "root" -> {
                    SheetRow(LucideIcons.Info, stringResource(R.string.home_about_app), { page = "about" }, chevron = true)
                    SheetRow(LucideIcons.Settings, stringResource(R.string.home_preferences), { page = "prefs" }, chevron = true)
                    SheetRow(LucideIcons.CircleHelp, stringResource(R.string.home_help_support), { page = "help" }, chevron = true)
                }
                "prefs" -> Preferences(vm)
                "about" -> About(vm)
                "help" -> {
                    val uri = LocalUriHandler.current
                    var leaving by remember { mutableStateOf<String?>(null) }
                    SheetRow(LucideIcons.CircleHelp, stringResource(R.string.home_help), { leaving = "$APP_REPO#readme" })
                    SheetRow(LucideIcons.MessageSquare, stringResource(R.string.home_report_problem), { leaving = "$APP_REPO/issues" })
                    leaving?.let { url ->
                        io.github.dikshant363.pdfcraft.ui.PdfDialog(
                            onDismiss = { leaving = null }, title = stringResource(R.string.home_leaving_title), confirm = stringResource(R.string.home_continue), onConfirm = { leaving = null; uri.openUri(url) },
                        ) { Text(stringResource(R.string.home_leaving_body), color = c.text, fontSize = 14.sp) }
                    }
                }
            }
        }
    }
}

@Composable
private fun Preferences(vm: AppViewModel) {
    val c = pdfColors
    var authorDialog by remember { mutableStateOf(false) }
    Section(stringResource(R.string.home_section_notifications))
    Text(stringResource(R.string.home_no_notifications), color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
    SheetDivider()
    Section(stringResource(R.string.home_section_suggestions))
    SheetRow(null, stringResource(R.string.home_offer_suggestions), { vm.updateFormSuggestions(!vm.formSuggestions) }) {
        PdfSwitch(vm.formSuggestions, { vm.updateFormSuggestions(it) })
    }
    SheetRow(null, stringResource(R.string.home_clear_suggestions), { vm.clearFormSuggestions() })
    SheetDivider()
    Section(stringResource(R.string.home_author_title))
    val authorHint = stringResource(R.string.home_author_hint)
    SheetRow(null, vm.toolPrefs.author.ifBlank { authorHint }, { authorDialog = true })
    SheetDivider()
    Section(stringResource(R.string.home_section_screen))
    SheetRow(null, stringResource(R.string.home_keep_screen_on), { vm.updateKeepScreenOn(!vm.keepScreenOn) }) {
        PdfSwitch(vm.keepScreenOn, { vm.updateKeepScreenOn(it) })
    }
    Text(stringResource(R.string.home_keep_screen_on_body), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
    SheetDivider()
    Section(stringResource(R.string.home_section_keyboard))
    SheetRow(null, stringResource(R.string.home_keyboard_shortcuts), { vm.updateKeyboardShortcuts(!vm.keyboardShortcuts) }) {
        PdfSwitch(vm.keyboardShortcuts, { vm.updateKeyboardShortcuts(it) })
    }
    Text(
        stringResource(R.string.home_keyboard_shortcuts_body),
        color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp),
    )
    SheetDivider()
    // what PdfCraft does differently from the reference, each by choice
    Section(stringResource(R.string.home_section_extra))
    Text(stringResource(R.string.home_extra_body), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
    SheetRow(null, stringResource(R.string.home_status_bar_in_documents), { vm.updateStatusBarInDocuments(!vm.statusBarInDocuments) }) {
        PdfSwitch(vm.statusBarInDocuments, { vm.updateStatusBarInDocuments(it) })
    }
    Text(stringResource(R.string.home_status_bar_in_documents_body), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
    Spacer(Modifier.height(8.dp))
    SheetRow(null, stringResource(R.string.home_search_count), { vm.updateSearchCount(!vm.searchCount) }) {
        PdfSwitch(vm.searchCount, { vm.updateSearchCount(it) })
    }
    Text(stringResource(R.string.home_search_count_body), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
    Spacer(Modifier.height(24.dp))
    if (authorDialog) {
        var name by remember { mutableStateOf(vm.toolPrefs.author) }
        io.github.dikshant363.pdfcraft.ui.PdfDialog(
            onDismiss = { authorDialog = false }, title = stringResource(R.string.home_author_title),
            confirm = stringResource(R.string.common_ok), onConfirm = { vm.toolPrefs.author = name.trim(); authorDialog = false; vm.prefsChanged++ },
        ) { io.github.dikshant363.pdfcraft.ui.PdfField(name, { name = it }, stringResource(R.string.common_your_name)) }
    }
}

/** PdfCraft for Android's own repository: its help, its issues and its source. */
private const val APP_REPO = "https://github.com/artcraft-android/pdfcraft-android"

@Composable
private fun Section(title: String) {
    Text(title, color = pdfColors.accent, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 20.dp, top = 16.dp, bottom = 4.dp))
}

@Composable
private fun About(vm: AppViewModel) {
    val c = pdfColors
    val clipboard = LocalClipboardManager.current
    val uri = LocalUriHandler.current
    Section(stringResource(R.string.home_section_about))
    val version = stringResource(R.string.home_version, vm.appVersion, io.github.dikshant363.pdfcraft.core.engineVersion())
    val copied = stringResource(R.string.home_version_copied)
    Text(
        version, color = c.text, fontSize = 16.sp,
        modifier = Modifier.fillMaxWidth()
            .pointerInput(version) { detectTapGestures(onLongPress = { clipboard.setText(AnnotatedString(version)); vm.tell(copied) }) }
            .padding(horizontal = 20.dp, vertical = 12.dp),
    )
    SheetDivider()
    Section(stringResource(R.string.home_section_legal))
    SheetRow(null, stringResource(R.string.home_licences), { vm.showLicences = true })
    SheetRow(null, stringResource(R.string.home_source_code), { uri.openUri(APP_REPO) })
    SheetRow(null, stringResource(R.string.home_engine), { uri.openUri("https://github.com/storytold/pdfcraft") })
    Text(
        stringResource(R.string.home_legal_notice),
        color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(20.dp), overflow = TextOverflow.Visible,
    )
}
