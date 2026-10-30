package com.storytold.pdfcraft.kit

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.NavigateBefore
import androidx.compose.material.icons.automirrored.filled.NavigateNext
import androidx.compose.material.icons.automirrored.filled.Redo
import androidx.compose.material.icons.automirrored.filled.Undo
import androidx.compose.material.icons.automirrored.outlined.FormatListBulleted
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.pdfcraft_ffi.*

enum class ViewerMode {
    VIEW,
    ANNOTATE,
    EDIT,
    REDACT
}

enum class AnnotationTool {
    PEN,
    HIGHLIGHTER,
    ERASER
}

/**
 * Unified high-level Jetpack Compose viewer for PdfCraft documents.
 * High-DPI crisp rendering, immersive reading mode, interactive AcroForm field editing,
 * link navigation, text selection actions, annotations, tool catalogue, and page grid.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PdfView(
    document: PdfDocument,
    initialPageIndex: Int = 0,
    initialAction: String? = null,
    onClose: (() -> Unit)? = null,
    onSave: (() -> Unit)? = null,
    onSaveAs: (() -> Unit)? = null,
    onShare: (() -> Unit)? = null,
    isDarkTheme: Boolean? = null,
    onToggleDarkTheme: (() -> Unit)? = null,
    modifier: Modifier = Modifier
) {
    val context = LocalContext.current
    val coroutineScope = rememberCoroutineScope()
    val prefs = remember { context.getSharedPreferences("pdfcraft_settings", Context.MODE_PRIVATE) }
    var storedThemeMode by remember {
        mutableStateOf(prefs.getString("app_theme_mode", "SYSTEM") ?: "SYSTEM")
    }
    val systemDark = isSystemInDarkTheme()
    val isDark = isDarkTheme ?: when (storedThemeMode) {
        "DARK" -> true
        "LIGHT" -> false
        else -> systemDark
    }

    var currentPage by remember { mutableStateOf(initialPageIndex) }
    var readingColors by remember { mutableStateOf<FfiReadingColors?>(null) }
    var hideComments by remember { mutableStateOf(false) }

    // Immersive Reading Mode
    var isChromeVisible by remember { mutableStateOf(true) }

    // Drawing & Markup State
    var viewerMode by remember { mutableStateOf(ViewerMode.VIEW) }
    var annotationTool by remember { mutableStateOf(AnnotationTool.PEN) }
    var activeInkColor by remember { mutableStateOf(0x339AF0u) }
    var activeInkWidth by remember { mutableFloatStateOf(3f) }
    var isInkMode by remember { mutableStateOf(false) }
    var isEraserMode by remember { mutableStateOf(false) }

    // Search state
    var isSearchActive by remember { mutableStateOf(false) }
    var searchQuery by remember { mutableStateOf("") }
    var searchMatches by remember { mutableStateOf<List<FfiFindMatch>>(emptyList()) }

    // Interactive Form Field Editing
    var activeEditField by remember { mutableStateOf<FfiFormField?>(null) }
    var editFieldInput by remember { mutableStateOf("") }
    var activeChoiceField by remember { mutableStateOf<FfiFormField?>(null) }

    // Text Selection Action Bar
    var selectedText by remember { mutableStateOf<String?>(null) }

    // Modals
    var showOutlineDialog by remember { mutableStateOf(false) }
    var showGoToPageDialog by remember { mutableStateOf(false) }
    var showPageGrid by remember { mutableStateOf(false) }
    var showCommandPalette by remember { mutableStateOf(false) }
    var showRedactionView by remember { mutableStateOf(false) }
    var showAboutView by remember { mutableStateOf(false) }
    var showExportDialog by remember { mutableStateOf(false) }
    var exportInitialTab by remember { mutableIntStateOf(0) }
    var exportInitialFormat by remember { mutableStateOf("docx") }
    var showMetadataDialog by remember { mutableStateOf(false) }
    var showSecurityDialog by remember { mutableStateOf(false) }
    var showWatermarkDialog by remember { mutableStateOf(false) }
    var activeTool by remember { mutableStateOf<FfiToolInfo?>(null) }
    var showMoreMenu by remember { mutableStateOf(false) }
    var statusNotification by remember { mutableStateOf<String?>(null) }
    var pdfPageViewRef by remember { mutableStateOf<PdfPageView?>(null) }
    var selectedContentItemState by remember { mutableStateOf<PdfAddedItem?>(null) }
    var showConfirmApplyRedactions by remember { mutableStateOf(false) }

    // Content Addition (Text & Image)
    var showAddTextDialog by remember { mutableStateOf(false) }
    var addTextInput by remember { mutableStateOf("") }
    var addTextFontSize by remember { mutableFloatStateOf(14f) }

    val insertImageLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.GetContent()
    ) { uri: Uri? ->
        if (uri != null) {
            try {
                val processedBytes = PdfImageProcessor.processImageUriToJpegBytes(context, uri)
                if (processedBytes != null) {
                    val tempFile = java.io.File(context.cacheDir, "insert_img_${System.currentTimeMillis()}.jpg")
                    tempFile.writeBytes(processedBytes)
                    val added = document.addImage(currentPage, tempFile.absolutePath)
                    statusNotification = "Image inserted — drag to move or resize"
                    viewerMode = ViewerMode.EDIT
                    selectedContentItemState = added
                    pdfPageViewRef?.isEditContentMode = true
                    pdfPageViewRef?.refreshPage()
                    if (added != null) {
                        pdfPageViewRef?.selectContentItem(added)
                    }
                } else {
                    statusNotification = "Could not decode selected image"
                }
            } catch (e: Exception) {
                statusNotification = "Failed to insert image: ${e.message}"
            }
        }
    }

    LaunchedEffect(initialAction) {
        when (initialAction) {
            "export_word" -> {
                exportInitialTab = 0
                exportInitialFormat = "docx"
                showExportDialog = true
            }
            "export_pptx" -> {
                exportInitialTab = 0
                exportInitialFormat = "pptx"
                showExportDialog = true
            }
            "export_images" -> {
                exportInitialTab = 1
                showExportDialog = true
            }
            "convert_pdfa" -> {
                exportInitialTab = 3
                showExportDialog = true
            }
            "add_text" -> {
                viewerMode = ViewerMode.EDIT
                showAddTextDialog = true
            }
            "insert_image" -> {
                viewerMode = ViewerMode.EDIT
                insertImageLauncher.launch("image/*")
            }
            "redact" -> {
                viewerMode = ViewerMode.REDACT
                pdfPageViewRef?.isRedactMode = true
                statusNotification = "Redact mode: Drag finger across area to mark"
            }
            "watermark" -> {
                showWatermarkDialog = true
            }
            "organize" -> {
                showPageGrid = true
            }
            "protect" -> {
                showSecurityDialog = true
            }
            "draw" -> {
                viewerMode = ViewerMode.ANNOTATE
                annotationTool = AnnotationTool.PEN
                isInkMode = true
                isEraserMode = false
                pdfPageViewRef?.isInkMode = true
                pdfPageViewRef?.isEraserMode = false
                pdfPageViewRef?.inkColorRgb = activeInkColor
                pdfPageViewRef?.inkStrokeWidth = activeInkWidth
            }
            "toc" -> {
                showOutlineDialog = true
            }
        }
    }

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

    MaterialTheme(
        colorScheme = if (isDark) darkColorScheme() else lightColorScheme()
    ) {
        Scaffold(
        topBar = {
            AnimatedVisibility(
                visible = isChromeVisible,
                enter = slideInVertically(initialOffsetY = { -it }) + fadeIn(),
                exit = slideOutVertically(targetOffsetY = { -it }) + fadeOut()
            ) {
                Column(modifier = Modifier.fillMaxWidth()) {
                    TopAppBar(
                        title = {
                            Column(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable { showGoToPageDialog = true }
                            ) {
                                Text(
                                    text = document.name.ifBlank { "Document" },
                                    style = MaterialTheme.typography.titleMedium,
                                    fontWeight = FontWeight.SemiBold,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis
                                )
                                Text(
                                    text = "Page ${currentPage + 1} of ${document.pageCount}",
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis
                                )
                            }
                        },
                        navigationIcon = {
                            onClose?.let { closeAction ->
                                IconButton(onClick = closeAction) {
                                    Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                                }
                            }
                        },
                        actions = {
                            // Search
                            IconButton(onClick = {
                                isSearchActive = !isSearchActive
                                if (!isSearchActive) searchQuery = ""
                            }) {
                                Icon(Icons.Outlined.Search, contentDescription = "Find in Document")
                            }

                            // Thumbnails / Grid
                            IconButton(onClick = { showPageGrid = true }) {
                                Icon(Icons.Outlined.GridView, contentDescription = "Thumbnails")
                            }

                            // More Menu
                            Box {
                                IconButton(onClick = { showMoreMenu = true }) {
                                    Icon(Icons.Filled.MoreVert, contentDescription = "More Options")
                                }
                                DropdownMenu(
                                    expanded = showMoreMenu,
                                    onDismissRequest = { showMoreMenu = false }
                                ) {
                                    onSave?.let { saveAction ->
                                        DropdownMenuItem(
                                            text = { Text("Save") },
                                            onClick = {
                                                showMoreMenu = false
                                                saveAction()
                                            }
                                        )
                                    }
                                    onSaveAs?.let { saveAsAction ->
                                        DropdownMenuItem(
                                            text = { Text("Save As...") },
                                            onClick = {
                                                showMoreMenu = false
                                                saveAsAction()
                                            }
                                        )
                                    }
                                    onShare?.let { shareAction ->
                                        DropdownMenuItem(
                                            text = { Text("Share PDF...") },
                                            onClick = {
                                                showMoreMenu = false
                                                shareAction()
                                            }
                                        )
                                    }
                                    DropdownMenuItem(
                                        text = { Text("Previous Page") },
                                        enabled = currentPage > 0,
                                        onClick = {
                                            showMoreMenu = false
                                            if (currentPage > 0) {
                                                currentPage--
                                                pdfPageViewRef?.setDocument(document, currentPage)
                                            }
                                        }
                                    )
                                    DropdownMenuItem(
                                        text = { Text("Next Page") },
                                        enabled = currentPage < document.pageCount - 1,
                                        onClick = {
                                            showMoreMenu = false
                                            if (currentPage < document.pageCount - 1) {
                                                currentPage++
                                                pdfPageViewRef?.setDocument(document, currentPage)
                                            }
                                        }
                                    )
                                    DropdownMenuItem(
                                        text = { Text("Go to Page...") },
                                        onClick = {
                                            showMoreMenu = false
                                            showGoToPageDialog = true
                                        }
                                    )
                                    DropdownMenuItem(
                                        text = { Text("Table of Contents / Outline...") },
                                        onClick = {
                                            showMoreMenu = false
                                            showOutlineDialog = true
                                        }
                                    )
                                    DropdownMenuItem(
                                        text = { Text("All Tools (Command Palette)...") },
                                        onClick = {
                                            showMoreMenu = false
                                            showCommandPalette = true
                                        }
                                    )
                                    HorizontalDivider()
                                    DropdownMenuItem(
                                        leadingIcon = {
                                            Icon(
                                                if (isDark) Icons.Outlined.LightMode else Icons.Outlined.DarkMode,
                                                contentDescription = null
                                            )
                                        },
                                        text = { Text(if (isDark) "Switch to Light Theme" else "Switch to Dark Theme") },
                                        onClick = {
                                            showMoreMenu = false
                                            val newMode = if (isDark) "LIGHT" else "DARK"
                                            storedThemeMode = newMode
                                            prefs.edit().putString("app_theme_mode", newMode).apply()
                                            onToggleDarkTheme?.invoke()
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
                                            viewerMode = ViewerMode.REDACT
                                            statusNotification = "Redact mode: Drag finger across area to mark"
                                        }
                                    )
                                    DropdownMenuItem(
                                        text = { Text("Add Text...") },
                                        onClick = {
                                            showMoreMenu = false
                                            showAddTextDialog = true
                                        }
                                    )
                                    DropdownMenuItem(
                                        text = { Text("Insert Image...") },
                                        onClick = {
                                            showMoreMenu = false
                                            insertImageLauncher.launch("image/*")
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
                                    leadingIcon = {
                                        Icon(
                                            Icons.Outlined.Search,
                                            contentDescription = null,
                                            modifier = Modifier.size(20.dp),
                                            tint = MaterialTheme.colorScheme.onSurfaceVariant
                                        )
                                    },
                                    trailingIcon = {
                                        if (searchQuery.isNotEmpty()) {
                                            IconButton(onClick = { searchQuery = "" }) {
                                                Icon(
                                                    Icons.Filled.Close,
                                                    contentDescription = "Clear search",
                                                    modifier = Modifier.size(18.dp)
                                                )
                                            }
                                        }
                                    },
                                    singleLine = true,
                                    textStyle = MaterialTheme.typography.bodyMedium,
                                    shape = RoundedCornerShape(10.dp),
                                    modifier = Modifier.weight(1f)
                                )
                                Spacer(modifier = Modifier.width(8.dp))
                                if (searchMatches.isNotEmpty()) {
                                    Text(
                                        text = "${searchMatches.size} on page",
                                        style = MaterialTheme.typography.labelSmall,
                                        color = MaterialTheme.colorScheme.primary,
                                        fontWeight = FontWeight.Medium
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
            }
        },
        bottomBar = {
            AnimatedVisibility(
                visible = isChromeVisible,
                enter = slideInVertically(initialOffsetY = { it }) + fadeIn(),
                exit = slideOutVertically(targetOffsetY = { it }) + fadeOut()
            ) {
                Surface(
                    tonalElevation = 6.dp,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    when (viewerMode) {
                        ViewerMode.VIEW -> {
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(horizontal = 12.dp, vertical = 6.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    FilledTonalButton(
                                        onClick = {
                                            viewerMode = ViewerMode.ANNOTATE
                                            annotationTool = AnnotationTool.PEN
                                            isInkMode = true
                                            isEraserMode = false
                                            pdfPageViewRef?.isInkMode = true
                                            pdfPageViewRef?.isEraserMode = false
                                            pdfPageViewRef?.inkColorRgb = activeInkColor
                                            pdfPageViewRef?.inkStrokeWidth = activeInkWidth
                                        },
                                        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
                                    ) {
                                        Icon(Icons.Outlined.Draw, contentDescription = null, modifier = Modifier.size(16.dp))
                                        Spacer(modifier = Modifier.width(6.dp))
                                        Text("Draw", style = MaterialTheme.typography.labelMedium)
                                    }
                                    FilledTonalButton(
                                        onClick = {
                                            viewerMode = ViewerMode.EDIT
                                            isInkMode = false
                                            isEraserMode = false
                                            pdfPageViewRef?.isInkMode = false
                                            pdfPageViewRef?.isEraserMode = false
                                        },
                                        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
                                    ) {
                                        Icon(Icons.Outlined.Edit, contentDescription = null, modifier = Modifier.size(16.dp))
                                        Spacer(modifier = Modifier.width(6.dp))
                                        Text("Edit", style = MaterialTheme.typography.labelMedium)
                                    }
                                    FilledTonalButton(
                                        onClick = { showPageGrid = true },
                                        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
                                    ) {
                                        Icon(Icons.Outlined.GridView, contentDescription = null, modifier = Modifier.size(16.dp))
                                        Spacer(modifier = Modifier.width(6.dp))
                                        Text("Pages", style = MaterialTheme.typography.labelMedium)
                                    }
                                }

                                Row(
                                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Surface(
                                        shape = RoundedCornerShape(12.dp),
                                        color = MaterialTheme.colorScheme.secondaryContainer,
                                        modifier = Modifier.clickable { showGoToPageDialog = true }
                                    ) {
                                        Text(
                                            text = "${currentPage + 1} / ${document.pageCount}",
                                            style = MaterialTheme.typography.labelMedium,
                                            fontWeight = FontWeight.Bold,
                                            color = MaterialTheme.colorScheme.onSecondaryContainer,
                                            modifier = Modifier.padding(horizontal = 8.dp, vertical = 6.dp)
                                        )
                                    }
                                    IconButton(onClick = { showOutlineDialog = true }) {
                                        Icon(Icons.AutoMirrored.Outlined.FormatListBulleted, contentDescription = "Outline")
                                    }
                                    IconButton(onClick = { isSearchActive = !isSearchActive }) {
                                        Icon(Icons.Outlined.Search, contentDescription = "Find")
                                    }
                                }
                            }
                        }
                        ViewerMode.ANNOTATE -> {
                            Column(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(horizontal = 12.dp, vertical = 8.dp)
                            ) {
                                Row(
                                    modifier = Modifier.fillMaxWidth(),
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Row(
                                        modifier = Modifier
                                            .weight(1f)
                                            .horizontalScroll(rememberScrollState()),
                                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                                        verticalAlignment = Alignment.CenterVertically
                                    ) {
                                        FilterChip(
                                            selected = annotationTool == AnnotationTool.PEN,
                                            onClick = {
                                                annotationTool = AnnotationTool.PEN
                                                isInkMode = true
                                                isEraserMode = false
                                                pdfPageViewRef?.isInkMode = true
                                                pdfPageViewRef?.isEraserMode = false
                                                pdfPageViewRef?.inkColorRgb = activeInkColor
                                                pdfPageViewRef?.inkStrokeWidth = activeInkWidth
                                            },
                                            leadingIcon = {
                                                Icon(Icons.Outlined.Draw, contentDescription = null, modifier = Modifier.size(16.dp))
                                            },
                                            label = { Text("Pen") }
                                        )
                                        FilterChip(
                                            selected = annotationTool == AnnotationTool.HIGHLIGHTER,
                                            onClick = {
                                                annotationTool = AnnotationTool.HIGHLIGHTER
                                                isInkMode = true
                                                isEraserMode = false
                                                pdfPageViewRef?.isInkMode = true
                                                pdfPageViewRef?.isEraserMode = false
                                                pdfPageViewRef?.inkColorRgb = 0xFFE066u
                                                pdfPageViewRef?.inkStrokeWidth = 16f
                                            },
                                            leadingIcon = {
                                                Icon(Icons.Outlined.Highlight, contentDescription = null, modifier = Modifier.size(16.dp))
                                            },
                                            label = { Text("Highlighter") }
                                        )
                                        FilterChip(
                                            selected = annotationTool == AnnotationTool.ERASER,
                                            onClick = {
                                                annotationTool = AnnotationTool.ERASER
                                                isEraserMode = true
                                                isInkMode = false
                                                pdfPageViewRef?.isEraserMode = true
                                                pdfPageViewRef?.isInkMode = false
                                            },
                                            leadingIcon = {
                                                Icon(Icons.Outlined.CleaningServices, contentDescription = null, modifier = Modifier.size(16.dp))
                                            },
                                            label = { Text("Eraser") }
                                        )
                                    }

                                    Spacer(modifier = Modifier.width(6.dp))

                                    Row(
                                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                                        verticalAlignment = Alignment.CenterVertically
                                    ) {
                                        IconButton(onClick = {
                                            try {
                                                val action = document.undo()
                                                statusNotification = "Undone: $action"
                                                pdfPageViewRef?.refreshPage()
                                            } catch (_: Throwable) {}
                                        }) {
                                            Icon(Icons.AutoMirrored.Filled.Undo, contentDescription = "Undo")
                                        }
                                        IconButton(onClick = {
                                            try {
                                                val action = document.redo()
                                                statusNotification = "Redone: $action"
                                                pdfPageViewRef?.refreshPage()
                                            } catch (_: Throwable) {}
                                        }) {
                                            Icon(Icons.AutoMirrored.Filled.Redo, contentDescription = "Redo")
                                        }
                                        Button(
                                            onClick = {
                                                viewerMode = ViewerMode.VIEW
                                                isInkMode = false
                                                isEraserMode = false
                                                pdfPageViewRef?.isInkMode = false
                                                pdfPageViewRef?.isEraserMode = false
                                            },
                                            contentPadding = PaddingValues(horizontal = 12.dp, vertical = 4.dp)
                                        ) {
                                            Icon(Icons.Filled.Check, contentDescription = null, modifier = Modifier.size(16.dp))
                                            Spacer(modifier = Modifier.width(4.dp))
                                            Text("Done")
                                        }
                                    }
                                }

                                Spacer(modifier = Modifier.height(8.dp))

                                Row(
                                    modifier = Modifier.fillMaxWidth(),
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    val colorList = listOf(
                                        0x339AF0u to Color(0xFF339AF0), // Blue
                                        0xFFE066u to Color(0xFFFFE066), // Yellow
                                        0x51CF66u to Color(0xFF51CF66), // Green
                                        0xFF6B6Bu to Color(0xFFFF6B6B), // Red
                                        0xFFA94Du to Color(0xFFFFA94D), // Orange
                                        0xCC5DE8u to Color(0xFFCC5DE8), // Purple
                                        0x212529u to Color(0xFF212529)  // Dark
                                    )
                                    Row(
                                        modifier = Modifier
                                            .weight(1f)
                                            .horizontalScroll(rememberScrollState()),
                                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                                    ) {
                                        colorList.forEach { (rgb, col) ->
                                            Box(
                                                modifier = Modifier
                                                    .size(28.dp)
                                                    .clip(CircleShape)
                                                    .background(col)
                                                    .border(
                                                        width = if (activeInkColor == rgb) 3.dp else 1.dp,
                                                        color = if (activeInkColor == rgb) MaterialTheme.colorScheme.primary else Color.Gray.copy(alpha = 0.5f),
                                                        shape = CircleShape
                                                    )
                                                    .clickable {
                                                        activeInkColor = rgb
                                                        pdfPageViewRef?.inkColorRgb = rgb
                                                    }
                                            )
                                        }
                                    }

                                    Spacer(modifier = Modifier.width(8.dp))

                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text("${activeInkWidth.toInt()}pt", style = MaterialTheme.typography.labelSmall)
                                        Slider(
                                            value = activeInkWidth,
                                            onValueChange = {
                                                activeInkWidth = it
                                                pdfPageViewRef?.inkStrokeWidth = it
                                            },
                                            valueRange = 1f..24f,
                                            modifier = Modifier.width(100.dp)
                                        )
                                    }
                                }
                            }
                        }
                        ViewerMode.EDIT -> {
                            if (selectedContentItemState != null) {
                                Row(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(horizontal = 12.dp, vertical = 6.dp),
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Row(
                                        modifier = Modifier.weight(1f, fill = false),
                                        verticalAlignment = Alignment.CenterVertically,
                                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                                    ) {
                                        Surface(
                                            shape = RoundedCornerShape(12.dp),
                                            color = MaterialTheme.colorScheme.primaryContainer
                                        ) {
                                            Text(
                                                text = if (selectedContentItemState?.type == "image") "Image" else "Text",
                                                style = MaterialTheme.typography.labelMedium,
                                                fontWeight = FontWeight.Bold,
                                                color = MaterialTheme.colorScheme.onPrimaryContainer,
                                                modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp)
                                            )
                                        }
                                        Text(
                                            text = "Drag to move • Corners to resize",
                                            style = MaterialTheme.typography.labelSmall,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                            maxLines = 1,
                                            overflow = TextOverflow.Ellipsis
                                        )
                                    }

                                    Row(
                                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                                        verticalAlignment = Alignment.CenterVertically
                                    ) {
                                        IconButton(onClick = {
                                            pdfPageViewRef?.deleteSelectedContent()
                                            selectedContentItemState = null
                                        }) {
                                            Icon(
                                                Icons.Outlined.Delete,
                                                contentDescription = "Delete",
                                                tint = MaterialTheme.colorScheme.error
                                            )
                                        }
                                        FilledTonalButton(
                                            onClick = {
                                                pdfPageViewRef?.selectContentItem(null)
                                                selectedContentItemState = null
                                            },
                                            contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
                                        ) {
                                            Icon(Icons.Filled.Check, contentDescription = null, modifier = Modifier.size(16.dp))
                                            Spacer(modifier = Modifier.width(4.dp))
                                            Text("Deselect", style = MaterialTheme.typography.labelMedium)
                                        }
                                    }
                                }
                            } else {
                                Row(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(horizontal = 12.dp, vertical = 6.dp),
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Row(
                                        modifier = Modifier
                                            .weight(1f)
                                            .horizontalScroll(rememberScrollState()),
                                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                                        verticalAlignment = Alignment.CenterVertically
                                    ) {
                                        FilledTonalButton(
                                            onClick = { showAddTextDialog = true },
                                            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp)
                                        ) {
                                            Icon(Icons.Outlined.TextFields, contentDescription = null, modifier = Modifier.size(16.dp))
                                            Spacer(modifier = Modifier.width(4.dp))
                                            Text("Text", style = MaterialTheme.typography.labelMedium)
                                        }
                                        FilledTonalButton(
                                            onClick = { insertImageLauncher.launch("image/*") },
                                            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp)
                                        ) {
                                            Icon(Icons.Outlined.AddPhotoAlternate, contentDescription = null, modifier = Modifier.size(16.dp))
                                            Spacer(modifier = Modifier.width(4.dp))
                                            Text("Image", style = MaterialTheme.typography.labelMedium)
                                        }
                                        FilledTonalButton(
                                            onClick = {
                                                viewerMode = ViewerMode.REDACT
                                                pdfPageViewRef?.isRedactMode = true
                                                statusNotification = "Redact mode: Drag finger across area to mark"
                                            },
                                            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp)
                                        ) {
                                            Icon(Icons.Outlined.VisibilityOff, contentDescription = null, modifier = Modifier.size(16.dp))
                                            Spacer(modifier = Modifier.width(4.dp))
                                            Text("Redact", style = MaterialTheme.typography.labelMedium)
                                        }
                                        FilledTonalButton(
                                            onClick = {
                                                try {
                                                    document.flatten(comments = true, fields = true)
                                                    statusNotification = "Document flattened"
                                                    pdfPageViewRef?.refreshPage()
                                                } catch (e: Exception) {
                                                    statusNotification = "Flatten error: ${e.message}"
                                                }
                                            },
                                            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp)
                                        ) {
                                            Icon(Icons.Outlined.LayersClear, contentDescription = null, modifier = Modifier.size(16.dp))
                                            Spacer(modifier = Modifier.width(4.dp))
                                            Text("Flatten", style = MaterialTheme.typography.labelMedium)
                                        }
                                    }
                                    Spacer(modifier = Modifier.width(6.dp))
                                    Button(
                                        onClick = {
                                            viewerMode = ViewerMode.VIEW
                                            pdfPageViewRef?.isEditContentMode = false
                                            pdfPageViewRef?.selectContentItem(null)
                                            selectedContentItemState = null
                                        },
                                        contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp)
                                    ) {
                                        Icon(Icons.Filled.Check, contentDescription = null, modifier = Modifier.size(16.dp))
                                        Spacer(modifier = Modifier.width(4.dp))
                                        Text("Done", style = MaterialTheme.typography.labelMedium)
                                    }
                                }
                            }
                        }
                        ViewerMode.REDACT -> {
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(horizontal = 12.dp, vertical = 6.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Row(
                                    modifier = Modifier.weight(1f, fill = false),
                                    verticalAlignment = Alignment.CenterVertically,
                                    horizontalArrangement = Arrangement.spacedBy(8.dp)
                                ) {
                                    Surface(
                                        shape = RoundedCornerShape(12.dp),
                                        color = MaterialTheme.colorScheme.errorContainer
                                    ) {
                                        Text(
                                            text = "Redact",
                                            style = MaterialTheme.typography.labelMedium,
                                            fontWeight = FontWeight.Bold,
                                            color = MaterialTheme.colorScheme.onErrorContainer,
                                            modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp)
                                        )
                                    }
                                    Text(
                                        text = "Drag finger over area to mark",
                                        style = MaterialTheme.typography.labelSmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis
                                    )
                                }

                                Row(
                                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Button(
                                        onClick = { showConfirmApplyRedactions = true },
                                        colors = ButtonDefaults.buttonColors(
                                            containerColor = MaterialTheme.colorScheme.error,
                                            contentColor = MaterialTheme.colorScheme.onError
                                        ),
                                        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
                                    ) {
                                        Icon(Icons.Outlined.VisibilityOff, contentDescription = null, modifier = Modifier.size(16.dp))
                                        Spacer(modifier = Modifier.width(4.dp))
                                        Text("Apply", style = MaterialTheme.typography.labelMedium)
                                    }
                                    FilledTonalButton(
                                        onClick = {
                                            viewerMode = ViewerMode.VIEW
                                            pdfPageViewRef?.isRedactMode = false
                                        },
                                        contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
                                    ) {
                                        Icon(Icons.Filled.Check, contentDescription = null, modifier = Modifier.size(16.dp))
                                        Spacer(modifier = Modifier.width(4.dp))
                                        Text("Done", style = MaterialTheme.typography.labelMedium)
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        modifier = modifier.fillMaxSize()
    ) { paddingValues ->
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.surfaceContainer)
                .padding(paddingValues)
        ) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    PdfPageView(ctx).apply {
                        setDocument(document, currentPage)
                        this.isDarkTheme = isDark
                        this.readingColors = readingColors
                        this.hideComments = hideComments
                        this.isInkMode = isInkMode
                        this.isEraserMode = isEraserMode
                        this.inkColorRgb = activeInkColor
                        this.inkStrokeWidth = activeInkWidth

                        this.onNavigateToPage = { targetPage ->
                            currentPage = targetPage
                            setDocument(document, targetPage)
                        }
                        this.onOpenUri = { uriStr ->
                            try {
                                val intent = Intent(Intent.ACTION_VIEW, Uri.parse(uriStr))
                                ctx.startActivity(intent)
                            } catch (e: Exception) {
                                statusNotification = "Could not open link: ${e.message}"
                            }
                        }
                        this.onEditTextField = { field ->
                            activeEditField = field
                            editFieldInput = field.value.firstOrNull() ?: ""
                        }
                        this.onSelectChoiceField = { field ->
                            activeChoiceField = field
                        }
                        this.onToggleChrome = {
                            isChromeVisible = !isChromeVisible
                        }
                        this.onTextSelected = { text ->
                            selectedText = text
                        }
                        this.onContentItemSelected = { item ->
                            selectedContentItemState = item
                        }
                        this.onContentItemUpdated = { item ->
                            selectedContentItemState = item
                            statusNotification = "Updated ${item.type} position"
                        }
                        this.onContentItemDeleted = { item ->
                            selectedContentItemState = null
                            statusNotification = "Deleted ${item.type}"
                        }
                        this.onRedactionMarked = {
                            statusNotification = "Area marked for redaction. Tap 'Apply' to permanently erase."
                        }

                        pdfPageViewRef = this
                    }
                },
                update = { view ->
                    pdfPageViewRef = view
                    if (view.document != document || view.pageIndex != currentPage) {
                        view.setDocument(document, currentPage)
                    }
                    view.isDarkTheme = isDark
                    view.readingColors = readingColors
                    view.hideComments = hideComments
                    view.isInkMode = isInkMode
                    view.isEraserMode = isEraserMode
                    view.inkColorRgb = activeInkColor
                    view.inkStrokeWidth = activeInkWidth
                    view.isEditContentMode = (viewerMode == ViewerMode.EDIT)
                    view.isRedactMode = (viewerMode == ViewerMode.REDACT)
                }
            )

            // Text Selection Action Bar (Floating at Top)
            selectedText?.let { textContent ->
                Surface(
                    shape = RoundedCornerShape(24.dp),
                    color = MaterialTheme.colorScheme.surfaceVariant,
                    shadowElevation = 8.dp,
                    modifier = Modifier
                        .align(Alignment.TopCenter)
                        .padding(top = 16.dp)
                ) {
                    Row(
                        modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        TextButton(onClick = {
                            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                            clipboard.setPrimaryClip(ClipData.newPlainText("PDF Text", textContent))
                            statusNotification = "Copied to clipboard"
                            pdfPageViewRef?.clearTextSelection()
                            selectedText = null
                        }) {
                            Text("Copy")
                        }
                        TextButton(onClick = {
                            pdfPageViewRef?.markupSelection(FfiTextMarkupKind.HIGHLIGHT, 0xFFE066u)
                            statusNotification = "Highlighted text"
                            selectedText = null
                        }) {
                            Text("Highlight")
                        }
                        TextButton(onClick = {
                            pdfPageViewRef?.markupSelection(FfiTextMarkupKind.UNDERLINE, 0x51CF66u)
                            statusNotification = "Underlined text"
                            selectedText = null
                        }) {
                            Text("Underline")
                        }
                        TextButton(onClick = {
                            pdfPageViewRef?.redactSelection()
                            statusNotification = "Text marked for redaction. Tap 'Apply' to permanently erase."
                            selectedText = null
                        }) {
                            Text("Redact", color = MaterialTheme.colorScheme.error)
                        }
                        TextButton(onClick = {
                            val shareIntent = Intent(Intent.ACTION_SEND).apply {
                                type = "text/plain"
                                putExtra(Intent.EXTRA_TEXT, textContent)
                            }
                            context.startActivity(Intent.createChooser(shareIntent, "Share text"))
                            pdfPageViewRef?.clearTextSelection()
                            selectedText = null
                        }) {
                            Text("Share")
                        }
                        IconButton(onClick = {
                            pdfPageViewRef?.clearTextSelection()
                            selectedText = null
                        }) {
                            Icon(Icons.Filled.Close, contentDescription = "Clear Selection", modifier = Modifier.size(18.dp))
                        }
                    }
                }
            }

            // Quick Floating Page Scrubber Pill (shown in immersive reading mode when chrome is hidden)
            if (!isChromeVisible) {
                Surface(
                    shape = RoundedCornerShape(20.dp),
                    color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.9f),
                    shadowElevation = 4.dp,
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .padding(bottom = 24.dp)
                        .clickable { showGoToPageDialog = true }
                ) {
                    Row(
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Text(
                            text = "${currentPage + 1} / ${document.pageCount}",
                            style = MaterialTheme.typography.labelMedium,
                            fontWeight = FontWeight.Bold,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                    }
                }
            }

            // Ephemeral notification pill
            statusNotification?.let { msg ->
                Surface(
                    shape = RoundedCornerShape(20.dp),
                    color = MaterialTheme.colorScheme.inverseSurface,
                    shadowElevation = 6.dp,
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .padding(bottom = 24.dp)
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

    // Form Field Text Edit Sheet
    activeEditField?.let { field ->
        ModalBottomSheet(
            onDismissRequest = { activeEditField = null }
        ) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 24.dp, vertical = 8.dp)
                    .navigationBarsPadding()
                    .imePadding()
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Text(
                        text = "Edit Form Field",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.Bold
                    )
                    TextButton(onClick = { activeEditField = null }) {
                        Text("Cancel")
                    }
                }
                Spacer(modifier = Modifier.height(4.dp))
                Text(
                    text = field.name,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.primary
                )
                Spacer(modifier = Modifier.height(12.dp))
                OutlinedTextField(
                    value = editFieldInput,
                    onValueChange = { editFieldInput = it },
                    label = { Text("Field Value") },
                    singleLine = false,
                    maxLines = 4,
                    modifier = Modifier.fillMaxWidth()
                )
                Spacer(modifier = Modifier.height(16.dp))
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(12.dp)
                ) {
                    OutlinedButton(
                        onClick = { editFieldInput = "" },
                        modifier = Modifier.weight(1f)
                    ) {
                        Text("Clear")
                    }
                    Button(
                        onClick = {
                            try {
                                document.setFormFieldValue(field.name, editFieldInput)
                                statusNotification = "Form field updated"
                                pdfPageViewRef?.refreshPage()
                            } catch (e: Exception) {
                                statusNotification = "Error updating field: ${e.message}"
                            }
                            activeEditField = null
                        },
                        modifier = Modifier.weight(1f)
                    ) {
                        Text("Save")
                    }
                }
                Spacer(modifier = Modifier.height(16.dp))
            }
        }
    }

    // Form Field Choice Selection Sheet
    activeChoiceField?.let { choiceField ->
        ModalBottomSheet(
            onDismissRequest = { activeChoiceField = null }
        ) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 24.dp, vertical = 8.dp)
                    .navigationBarsPadding()
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Text(
                        text = "Select Option",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.Bold
                    )
                    TextButton(onClick = { activeChoiceField = null }) {
                        Text("Close")
                    }
                }
                Text(
                    text = choiceField.name,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.secondary
                )
                Spacer(modifier = Modifier.height(12.dp))
                LazyColumn(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(max = 300.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp)
                ) {
                    items(choiceField.options) { opt ->
                        val isSelected = choiceField.value.contains(opt.exportValue)
                        Surface(
                            shape = RoundedCornerShape(10.dp),
                            color = if (isSelected) {
                                MaterialTheme.colorScheme.primaryContainer
                            } else {
                                MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f)
                            },
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    try {
                                        document.setFormFieldValue(choiceField.name, opt.exportValue)
                                        statusNotification = "Selected ${opt.displayText.ifEmpty { opt.exportValue }}"
                                        pdfPageViewRef?.refreshPage()
                                    } catch (_: Exception) {}
                                    activeChoiceField = null
                                }
                        ) {
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(horizontal = 16.dp, vertical = 12.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Text(
                                    text = opt.displayText.ifEmpty { opt.exportValue },
                                    style = MaterialTheme.typography.bodyMedium,
                                    fontWeight = if (isSelected) FontWeight.Bold else FontWeight.Normal,
                                    color = if (isSelected) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface
                                )
                                if (isSelected) {
                                    Icon(
                                        imageVector = Icons.Filled.Check,
                                        contentDescription = "Selected",
                                        tint = MaterialTheme.colorScheme.primary,
                                        modifier = Modifier.size(18.dp)
                                    )
                                }
                            }
                        }
                    }
                }
                Spacer(modifier = Modifier.height(16.dp))
            }
        }
    }

    // Page Grid Sheet
    if (showPageGrid) {
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(
            onDismissRequest = { showPageGrid = false },
            sheetState = sheetState
        ) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .fillMaxHeight(0.85f)
            ) {
                PdfPageGridView(
                    document = document,
                    onPageSelected = { selectedPage ->
                        coroutineScope.launch {
                            try {
                                sheetState.hide()
                            } catch (_: Throwable) {
                            } finally {
                                currentPage = selectedPage.coerceIn(0, (document.pageCount - 1).coerceAtLeast(0))
                                pdfPageViewRef?.setDocument(document, currentPage)
                                showPageGrid = false
                            }
                        }
                    }
                )
            }
        }
    }

    // Command Palette Sheet
    if (showCommandPalette) {
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(
            onDismissRequest = { showCommandPalette = false },
            sheetState = sheetState
        ) {
            PdfCommandPaletteView(
                engine = document.engine,
                onSelectTool = { tool ->
                    coroutineScope.launch {
                        try {
                            sheetState.hide()
                        } catch (_: Throwable) {
                        } finally {
                            showCommandPalette = false
                            activeTool = tool
                        }
                    }
                },
                onDismiss = {
                    coroutineScope.launch {
                        try {
                            sheetState.hide()
                        } catch (_: Throwable) {
                        } finally {
                            showCommandPalette = false
                        }
                    }
                }
            )
        }
    }

    // Tool Form Sheet
    activeTool?.let { tool ->
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(
            onDismissRequest = { activeTool = null },
            sheetState = sheetState
        ) {
            PdfToolFormView(
                tool = tool,
                document = document,
                engine = document.engine,
                onComplete = {
                    coroutineScope.launch {
                        try {
                            sheetState.hide()
                        } catch (_: Throwable) {
                        } finally {
                            activeTool = null
                            pdfPageViewRef?.refreshPage()
                        }
                    }
                },
                onDismiss = {
                    coroutineScope.launch {
                        try {
                            sheetState.hide()
                        } catch (_: Throwable) {
                        } finally {
                            activeTool = null
                        }
                    }
                }
            )
        }
    }

    // Redaction Sheet
    if (showRedactionView) {
        ModalBottomSheet(onDismissRequest = { showRedactionView = false }) {
            PdfRedactionView(
                document = document,
                pageIndex = currentPage,
                onDismiss = { showRedactionView = false },
                onApplied = {
                    showRedactionView = false
                    pdfPageViewRef?.refreshPage()
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

    // Export Sheet
    if (showExportDialog) {
        ModalBottomSheet(onDismissRequest = { showExportDialog = false }) {
            PdfExportDialog(
                document = document,
                initialTab = exportInitialTab,
                initialOfficeFormat = exportInitialFormat,
                onDismiss = { showExportDialog = false },
                onExportOffice = { _, outPath ->
                    try {
                        document.exportOffice(outPath)
                        statusNotification = "Office document exported to $outPath"
                        showExportDialog = false
                    } catch (e: Exception) {
                        statusNotification = "Export error: ${e.message}"
                    }
                },
                onExportText = { outPath ->
                    try {
                        document.exportText(outPath)
                        statusNotification = "Text exported to $outPath"
                        showExportDialog = false
                    } catch (e: Exception) {
                        statusNotification = "Export error: ${e.message}"
                    }
                },
                onExportImages = { outDir, format, dpi ->
                    try {
                        document.exportImages(outDir, format, dpi)
                        statusNotification = "Images exported to $outDir"
                        showExportDialog = false
                    } catch (e: Exception) {
                        statusNotification = "Export error: ${e.message}"
                    }
                },
                onExtractAllImages = { outDir ->
                    try {
                        document.extractAllImages(outDir)
                        statusNotification = "Images extracted to $outDir"
                        showExportDialog = false
                    } catch (e: Exception) {
                        statusNotification = "Extract error: ${e.message}"
                    }
                },
                onConvertToPdfA = { level ->
                    try {
                        val fixed = document.convertToPdfA(level)
                        statusNotification = "Converted to PDF/A ($fixed repairs applied)"
                        showExportDialog = false
                        pdfPageViewRef?.refreshPage()
                    } catch (e: Exception) {
                        statusNotification = "PDF/A error: ${e.message}"
                    }
                },
                onFlatten = { comments, fields ->
                    try {
                        document.flatten(comments, fields)
                        statusNotification = "Document flattened successfully"
                        showExportDialog = false
                        pdfPageViewRef?.refreshPage()
                    } catch (e: Exception) {
                        statusNotification = "Flatten error: ${e.message}"
                    }
                }
            )
        }
    }

    // Add Text Dialog
    if (showAddTextDialog) {
        AlertDialog(
            onDismissRequest = { showAddTextDialog = false },
            title = { Text("Add Text to Page") },
            text = {
                Column {
                    Text("Enter text to place onto the current page:", style = MaterialTheme.typography.bodyMedium)
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = addTextInput,
                        onValueChange = { addTextInput = it },
                        label = { Text("Text content") },
                        modifier = Modifier.fillMaxWidth()
                    )
                    Spacer(modifier = Modifier.height(12.dp))
                    Text("Font size: ${addTextFontSize.toInt()} pt", style = MaterialTheme.typography.labelMedium)
                    Slider(
                        value = addTextFontSize,
                        onValueChange = { addTextFontSize = it },
                        valueRange = 8f..36f,
                        steps = 6
                    )
                }
            },
            confirmButton = {
                Button(
                    onClick = {
                        if (addTextInput.isNotBlank()) {
                            try {
                                val added = document.addPageText(
                                    page = currentPage,
                                    text = addTextInput,
                                    atX = 72f,
                                    atY = 100f,
                                    fontSize = addTextFontSize
                                )
                                statusNotification = "Text added — drag to move or resize"
                                showAddTextDialog = false
                                addTextInput = ""
                                viewerMode = ViewerMode.EDIT
                                selectedContentItemState = added
                                pdfPageViewRef?.isEditContentMode = true
                                pdfPageViewRef?.refreshPage()
                                if (added != null) {
                                    pdfPageViewRef?.selectContentItem(added)
                                }
                            } catch (e: Exception) {
                                statusNotification = "Add text error: ${e.message}"
                            }
                        }
                    }
                ) {
                    Text("Add")
                }
            },
            dismissButton = {
                TextButton(onClick = { showAddTextDialog = false }) {
                    Text("Cancel")
                }
            }
        )
    }

    // Metadata Sheet
    if (showMetadataDialog) {
        val info = remember {
            try { document.engine.docInfo(document.docId) } catch (_: Exception) { null }
        }
        ModalBottomSheet(onDismissRequest = { showMetadataDialog = false }) {
            PdfMetadataDialog(
                initialTitle = info?.title ?: "",
                initialAuthor = info?.author ?: "",
                initialSubject = info?.subject ?: "",
                initialKeywords = info?.keywords ?: "",
                onDismiss = { showMetadataDialog = false },
                onSave = { t, a, s, k ->
                    try {
                        document.setMetadata(t, a, s, k)
                        statusNotification = "Metadata updated"
                        showMetadataDialog = false
                    } catch (e: Exception) {
                        statusNotification = "Metadata error: ${e.message}"
                    }
                }
            )
        }
    }

    // Security Sheet
    if (showSecurityDialog) {
        ModalBottomSheet(onDismissRequest = { showSecurityDialog = false }) {
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

    // Watermark & Header/Footer Sheet
    if (showWatermarkDialog) {
        ModalBottomSheet(onDismissRequest = { showWatermarkDialog = false }) {
            PdfWatermarkDialog(
                onDismiss = { showWatermarkDialog = false },
                onAddWatermark = { text, opacity, rotation ->
                    try {
                        document.addWatermark(text, opacity, rotation)
                        statusNotification = "Watermark added"
                        pdfPageViewRef?.refreshPage()
                    } catch (e: Exception) {
                        statusNotification = "Watermark error: ${e.message}"
                    }
                },
                onAddHeaderFooter = { hl, hc, hr, fl, fc, fr, shrink ->
                    try {
                        document.addHeaderFooter(hl, hc, hr, fl, fc, fr, shrink)
                        statusNotification = "Header & Footer added"
                        pdfPageViewRef?.refreshPage()
                    } catch (e: Exception) {
                        statusNotification = "Header/Footer error: ${e.message}"
                    }
                }
            )
        }
    }

    // Outline / TOC Sheet
    if (showOutlineDialog) {
        val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
        ModalBottomSheet(
            onDismissRequest = { showOutlineDialog = false },
            sheetState = sheetState
        ) {
            PdfOutlineDialog(
                outline = try { document.outline() } catch (_: Throwable) { emptyList() },
                currentPage = currentPage,
                onPageSelected = { selected ->
                    coroutineScope.launch {
                        try {
                            sheetState.hide()
                        } catch (_: Throwable) {
                        } finally {
                            currentPage = selected.coerceIn(0, (document.pageCount - 1).coerceAtLeast(0))
                            pdfPageViewRef?.setDocument(document, currentPage)
                            showOutlineDialog = false
                        }
                    }
                },
                onDismiss = {
                    coroutineScope.launch {
                        try {
                            sheetState.hide()
                        } catch (_: Throwable) {
                        } finally {
                            showOutlineDialog = false
                        }
                    }
                }
            )
        }
    }

    // Go To Page Dialog
    if (showGoToPageDialog) {
        Dialog(onDismissRequest = { showGoToPageDialog = false }) {
            PdfGoToPageDialog(
                currentPage = currentPage,
                pageCount = document.pageCount,
                onPageSelected = { selected ->
                    currentPage = selected
                    pdfPageViewRef?.setDocument(document, selected)
                    showGoToPageDialog = false
                },
                onDismiss = { showGoToPageDialog = false }
            )
        }
    }

    // Confirm Apply Redactions Dialog
    if (showConfirmApplyRedactions) {
        AlertDialog(
            onDismissRequest = { showConfirmApplyRedactions = false },
            icon = {
                Icon(
                    Icons.Outlined.Warning,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.error
                )
            },
            title = {
                Text("Apply Redactions Permanently?")
            },
            text = {
                Text("All marked areas on this document will be permanently erased. Text, vectors, and bitmap pixels under the redaction marks will be removed and sanitized. This action cannot be undone.")
            },
            confirmButton = {
                Button(
                    onClick = {
                        showConfirmApplyRedactions = false
                        coroutineScope.launch(Dispatchers.IO) {
                            try {
                                document.applyRedactions()
                                withContext(Dispatchers.Main) {
                                    statusNotification = "Redactions applied permanently"
                                    pdfPageViewRef?.refreshPage()
                                    viewerMode = ViewerMode.VIEW
                                    pdfPageViewRef?.isRedactMode = false
                                }
                            } catch (e: Exception) {
                                withContext(Dispatchers.Main) {
                                    statusNotification = "Apply redactions failed: ${e.message}"
                                }
                            }
                        }
                    },
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                        contentColor = MaterialTheme.colorScheme.onError
                    )
                ) {
                    Text("Apply & Erase")
                }
            },
            dismissButton = {
                TextButton(onClick = { showConfirmApplyRedactions = false }) {
                    Text("Cancel")
                }
            }
        )
    }
    }
}
