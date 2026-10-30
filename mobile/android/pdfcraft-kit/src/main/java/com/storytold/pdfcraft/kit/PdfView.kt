package com.storytold.pdfcraft.kit

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import uniffi.pdfcraft_ffi.*

/**
 * Unified high-level Jetpack Compose viewer for PdfCraft documents (Phase 2 MVP).
 * Combines native tiled rendering, gesture compositing, search, page navigation,
 * annotations toolbar, tool catalogue, and page grid operations.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PdfView(
    document: PdfDocument,
    initialPageIndex: Int = 0,
    onClose: (() -> Unit)? = null,
    modifier: Modifier = Modifier
) {
    var currentPage by remember { mutableStateOf(initialPageIndex) }
    var readingColors by remember { mutableStateOf<FfiReadingColors?>(null) }
    var hideComments by remember { mutableStateOf(false) }

    // Search state
    var isSearchActive by remember { mutableStateOf(false) }
    var searchQuery by remember { mutableStateOf("") }
    var searchMatches by remember { mutableStateOf<List<FfiFindMatch>>(emptyList()) }
    // Modals
    var showPageGrid by remember { mutableStateOf(false) }
    var showCommandPalette by remember { mutableStateOf(false) }
    var showRedactionView by remember { mutableStateOf(false) }
    var showAboutView by remember { mutableStateOf(false) }
    var showExportDialog by remember { mutableStateOf(false) }
    var showMetadataDialog by remember { mutableStateOf(false) }
    var showSecurityDialog by remember { mutableStateOf(false) }
    var showWatermarkDialog by remember { mutableStateOf(false) }
    var activeTool by remember { mutableStateOf<FfiToolInfo?>(null) }
    var showMoreMenu by remember { mutableStateOf(false) }
    var statusNotification by remember { mutableStateOf<String?>(null) }

    var pdfPageViewRef by remember { mutableStateOf<PdfPageView?>(null) }

    fun refreshPageHighlights() {
        val view = pdfPageViewRef ?: return
        val currentMatches = document.find(currentPage, searchQuery)
        searchMatches = currentMatches
        view.searchHighlights = currentMatches.map { it.rect }
    }

    LaunchedEffect(currentPage, searchQuery, isSearchActive) {
        if (isSearchActive && searchQuery.isNotBlank()) {
            refreshPageHighlights()
        } else {
            pdfPageViewRef?.searchHighlights = emptyList()
            searchMatches = emptyList()
        }
    }

    Scaffold(
        topBar = {
            Column(modifier = Modifier.fillMaxWidth()) {
                TopAppBar(
                    title = {
                        Column {
                            Text(
                                text = document.name,
                                style = MaterialTheme.typography.titleMedium,
                                fontWeight = FontWeight.SemiBold,
                                maxLines = 1
                            )
                            Text(
                                text = "Page ${currentPage + 1} of ${document.pageCount}",
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.secondary
                            )
                        }
                    },
                    navigationIcon = {
                        onClose?.let { closeAction ->
                            TextButton(onClick = closeAction) {
                                Text("Close")
                            }
                        }
                    },
                    actions = {
                        // Previous / Next page
                        IconButton(
                            onClick = {
                                if (currentPage > 0) {
                                    currentPage--
                                    pdfPageViewRef?.setDocument(document, currentPage)
                                }
                            },
                            enabled = currentPage > 0
                        ) {
                            Text("<", fontWeight = FontWeight.Bold)
                        }

                        IconButton(
                            onClick = {
                                if (currentPage < document.pageCount - 1) {
                                    currentPage++
                                    pdfPageViewRef?.setDocument(document, currentPage)
                                }
                            },
                            enabled = currentPage < document.pageCount - 1
                        ) {
                            Text(">", fontWeight = FontWeight.Bold)
                        }

                        // Thumbnails / Grid
                        IconButton(onClick = { showPageGrid = true }) {
                            Text("Grid", style = MaterialTheme.typography.labelSmall)
                        }

                        // Search
                        IconButton(onClick = {
                            isSearchActive = !isSearchActive
                            if (!isSearchActive) searchQuery = ""
                        }) {
                            Text("Find", style = MaterialTheme.typography.labelSmall)
                        }

                        // Tools
                        IconButton(onClick = { showCommandPalette = true }) {
                            Text("Tools", style = MaterialTheme.typography.labelSmall)
                        }

                        // More Menu
                        Box {
                            IconButton(onClick = { showMoreMenu = true }) {
                                Text("⋮", fontWeight = FontWeight.Bold)
                            }
                            DropdownMenu(
                                expanded = showMoreMenu,
                                onDismissRequest = { showMoreMenu = false }
                            ) {
                                DropdownMenuItem(
                                    text = { Text(if (readingColors == null) "Dark Mode" else "Normal Mode") },
                                    onClick = {
                                        showMoreMenu = false
                                        readingColors = if (readingColors == null) {
                                            FfiReadingColors(
                                                mode = FfiReadingColorsMode.WHITE_ON_BLACK,
                                                customTextRgb = null,
                                                customBackgroundRgb = null,
                                                onlyBlack = false,
                                                lineArt = true
                                            )
                                        } else {
                                            null
                                        }
                                        pdfPageViewRef?.readingColors = readingColors
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text(if (hideComments) "Show Comments" else "Hide Comments") },
                                    onClick = {
                                        showMoreMenu = false
                                        hideComments = !hideComments
                                        pdfPageViewRef?.hideComments = hideComments
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("Redact Content...") },
                                    onClick = {
                                        showMoreMenu = false
                                        showRedactionView = true
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("Export & Convert...") },
                                    onClick = {
                                        showMoreMenu = false
                                        showExportDialog = true
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("Document Properties...") },
                                    onClick = {
                                        showMoreMenu = false
                                        showMetadataDialog = true
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("Password & Security...") },
                                    onClick = {
                                        showMoreMenu = false
                                        showSecurityDialog = true
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("Watermark & Headers...") },
                                    onClick = {
                                        showMoreMenu = false
                                        showWatermarkDialog = true
                                    }
                                )
                                DropdownMenuItem(
                                    text = { Text("About PdfCraft") },
                                    onClick = {
                                        showMoreMenu = false
                                        showAboutView = true
                                    }
                                )
                            }
                        }
                    }
                )

                // Search Bar
                if (isSearchActive) {
                    Surface(
                        tonalElevation = 3.dp,
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(horizontal = 16.dp, vertical = 6.dp),
                            verticalAlignment = Alignment.CenterVertically
                        ) {
                            OutlinedTextField(
                                value = searchQuery,
                                onValueChange = { searchQuery = it },
                                placeholder = { Text("Find in document...") },
                                singleLine = true,
                                modifier = Modifier
                                    .weight(1f)
                                    .height(48.dp)
                            )
                            Spacer(modifier = Modifier.width(8.dp))
                            if (searchMatches.isNotEmpty()) {
                                Text(
                                    text = "${searchMatches.size} on page",
                                    style = MaterialTheme.typography.labelSmall
                                )
                                Spacer(modifier = Modifier.width(8.dp))
                            }
                            TextButton(onClick = {
                                isSearchActive = false
                                searchQuery = ""
                            }) {
                                Text("Done")
                            }
                        }
                    }
                }
            }
        },
        bottomBar = {
            Surface(
                tonalElevation = 4.dp,
                modifier = Modifier.fillMaxWidth()
            ) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                    horizontalArrangement = Arrangement.SpaceAround,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    // Highlight
                    Button(
                        onClick = {
                            pdfPageViewRef?.markupSelection(
                                kind = FfiTextMarkupKind.HIGHLIGHT,
                                colorRgb = 0xFFE066u
                            )
                            statusNotification = "Highlighted selection"
                        },
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp)
                    ) {
                        Text("Highlight", style = MaterialTheme.typography.labelSmall)
                    }

                    // Underline
                    Button(
                        onClick = {
                            pdfPageViewRef?.markupSelection(
                                kind = FfiTextMarkupKind.UNDERLINE,
                                colorRgb = 0x51CF66u
                            )
                            statusNotification = "Underlined selection"
                        },
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp)
                    ) {
                        Text("Underline", style = MaterialTheme.typography.labelSmall)
                    }

                    // Strikeout
                    Button(
                        onClick = {
                            pdfPageViewRef?.markupSelection(
                                kind = FfiTextMarkupKind.STRIKE_OUT,
                                colorRgb = 0xFF6B6Bu
                            )
                            statusNotification = "Strikethrough selection"
                        },
                        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp)
                    ) {
                        Text("Strikeout", style = MaterialTheme.typography.labelSmall)
                    }

                    // Undo
                    IconButton(onClick = {
                        try {
                            val action = document.undo()
                            statusNotification = "Undone: $action"
                            pdfPageViewRef?.setDocument(document, currentPage)
                        } catch (_: Throwable) {}
                    }) {
                        Text("↶", fontWeight = FontWeight.Bold)
                    }

                    // Redo
                    IconButton(onClick = {
                        try {
                            val action = document.redo()
                            statusNotification = "Redone: $action"
                            pdfPageViewRef?.setDocument(document, currentPage)
                        } catch (_: Throwable) {}
                    }) {
                        Text("↷", fontWeight = FontWeight.Bold)
                    }
                }
            }
        },
        modifier = modifier.fillMaxSize()
    ) { paddingValues ->
        Box(
            modifier = Modifier
                .fillMaxSize()
                .padding(paddingValues)
        ) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    PdfPageView(ctx).apply {
                        setDocument(document, currentPage)
                        this.readingColors = readingColors
                        this.hideComments = hideComments
                        pdfPageViewRef = this
                    }
                },
                update = { view ->
                    pdfPageViewRef = view
                    if (view.document != document || view.pageIndex != currentPage) {
                        view.setDocument(document, currentPage)
                    }
                    view.readingColors = readingColors
                    view.hideComments = hideComments
                }
            )

            // Ephemeral notification pill
            statusNotification?.let { msg ->
                Surface(
                    shape = RoundedCornerShape(20.dp),
                    color = MaterialTheme.colorScheme.inverseSurface,
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .padding(bottom = 16.dp)
                ) {
                    Text(
                        text = msg,
                        color = MaterialTheme.colorScheme.inverseOnSurface,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp)
                    )
                }
            }
        }
    }

    // Page Grid Sheet
    if (showPageGrid) {
        Dialog(onDismissRequest = { showPageGrid = false }) {
            Surface(
                shape = RoundedCornerShape(16.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .fillMaxHeight(0.85f)
            ) {
                PdfPageGridView(
                    document = document,
                    onPageSelected = { selectedPage ->
                        currentPage = selectedPage
                        pdfPageViewRef?.setDocument(document, selectedPage)
                        showPageGrid = false
                    }
                )
            }
        }
    }

    // Command Palette Sheet
    if (showCommandPalette) {
        Dialog(onDismissRequest = { showCommandPalette = false }) {
            PdfCommandPaletteView(
                engine = document.engine,
                onSelectTool = { tool ->
                    showCommandPalette = false
                    activeTool = tool
                },
                onDismiss = { showCommandPalette = false }
            )
        }
    }

    // Tool Form Dialog
    activeTool?.let { tool ->
        Dialog(onDismissRequest = { activeTool = null }) {
            PdfToolFormView(
                tool = tool,
                document = document,
                engine = document.engine,
                onComplete = {
                    pdfPageViewRef?.setDocument(document, currentPage)
                },
                onDismiss = { activeTool = null }
            )
        }
    }

    // Redaction Sheet
    if (showRedactionView) {
        Dialog(onDismissRequest = { showRedactionView = false }) {
            PdfRedactionView(
                document = document,
                pageIndex = currentPage,
                onDismiss = { showRedactionView = false },
                onApplied = {
                    showRedactionView = false
                    pdfPageViewRef?.setDocument(document, currentPage)
                }
            )
        }
    }

    // About Dialog
    if (showAboutView) {
        Dialog(onDismissRequest = { showAboutView = false }) {
            PdfAboutView(onDismiss = { showAboutView = false })
        }
    }

    // Export & Convert Dialog
    if (showExportDialog) {
        Dialog(onDismissRequest = { showExportDialog = false }) {
            PdfExportDialog(
                document = document,
                onDismiss = { showExportDialog = false },
                onExportText = { path ->
                    try {
                        document.exportText(path)
                        statusNotification = "Text exported to $path"
                    } catch (e: Exception) {
                        statusNotification = "Export text error: ${e.message}"
                    }
                    showExportDialog = false
                },
                onExportImages = { outDir, format, dpi ->
                    try {
                        document.exportImages(outDir, format, dpi)
                        statusNotification = "Images exported to $outDir"
                    } catch (e: Exception) {
                        statusNotification = "Export images error: ${e.message}"
                    }
                    showExportDialog = false
                },
                onConvertToPdfA = { level ->
                    try {
                        val count = document.convertToPdfA(level)
                        statusNotification = "Converted to $level ($count issues fixed)"
                        pdfPageViewRef?.setDocument(document, currentPage)
                    } catch (e: Exception) {
                        statusNotification = "Convert to PDF/A error: ${e.message}"
                    }
                    showExportDialog = false
                },
                onFlatten = { comments, fields ->
                    try {
                        document.flatten(comments, fields)
                        statusNotification = "Document flattened"
                        pdfPageViewRef?.setDocument(document, currentPage)
                    } catch (e: Exception) {
                        statusNotification = "Flatten error: ${e.message}"
                    }
                    showExportDialog = false
                }
            )
        }
    }

    // Document Properties & Metadata Dialog
    if (showMetadataDialog) {
        Dialog(onDismissRequest = { showMetadataDialog = false }) {
            val info = try { document.engine.docInfo(document.docId) } catch (_: Exception) { null }
            PdfMetadataDialog(
                initialTitle = info?.title ?: "",
                initialAuthor = info?.author ?: "",
                initialSubject = info?.subject ?: "",
                initialKeywords = info?.keywords ?: "",
                onDismiss = { showMetadataDialog = false },
                onSave = { title, author, subject, keywords ->
                    try {
                        document.setMetadata(title, author, subject, keywords)
                        statusNotification = "Metadata updated"
                    } catch (e: Exception) {
                        statusNotification = "Metadata error: ${e.message}"
                    }
                }
            )
        }
    }

    // Security & Password Protection Dialog
    if (showSecurityDialog) {
        Dialog(onDismissRequest = { showSecurityDialog = false }) {
            PdfSecurityDialog(
                onDismiss = { showSecurityDialog = false },
                onProtect = { open, perm, printing, changes, copy, a11y ->
                    try {
                        document.protect(open, perm, printing, changes, copy, a11y)
                        statusNotification = "Document encrypted & protected"
                    } catch (e: Exception) {
                        statusNotification = "Protection error: ${e.message}"
                    }
                },
                onUnprotect = {
                    try {
                        document.unprotect()
                        statusNotification = "Password security removed"
                    } catch (e: Exception) {
                        statusNotification = "Remove security error: ${e.message}"
                    }
                }
            )
        }
    }

    // Watermark & Header/Footer Dialog
    if (showWatermarkDialog) {
        Dialog(onDismissRequest = { showWatermarkDialog = false }) {
            PdfWatermarkDialog(
                onDismiss = { showWatermarkDialog = false },
                onAddWatermark = { text, opacity, rotation ->
                    try {
                        document.addWatermark(text, opacity, rotation)
                        statusNotification = "Watermark added"
                        pdfPageViewRef?.setDocument(document, currentPage)
                    } catch (e: Exception) {
                        statusNotification = "Watermark error: ${e.message}"
                    }
                },
                onAddHeaderFooter = { hl, hc, hr, fl, fc, fr, shrink ->
                    try {
                        document.addHeaderFooter(hl, hc, hr, fl, fc, fr, shrink)
                        statusNotification = "Header & Footer added"
                        pdfPageViewRef?.setDocument(document, currentPage)
                    } catch (e: Exception) {
                        statusNotification = "Header/Footer error: ${e.message}"
                    }
                }
            )
        }
    }
}
