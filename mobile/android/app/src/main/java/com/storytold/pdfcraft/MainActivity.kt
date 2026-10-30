package com.storytold.pdfcraft

import android.content.ComponentCallbacks2
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.BrandingWatermark
import androidx.compose.material.icons.automirrored.outlined.FormatListBulleted
import androidx.compose.material.icons.automirrored.outlined.NoteAdd
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Handyman
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import com.storytold.pdfcraft.kit.PdfAboutView
import com.storytold.pdfcraft.kit.PdfDocument
import com.storytold.pdfcraft.kit.PdfView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

data class RecentDocumentItem(
    val uriString: String,
    val name: String,
    val timestamp: Long
)

object RecentDocumentsManager {
    private const val PREFS_NAME = "pdfcraft_recents"
    private const val KEY_LIST = "recent_documents"

    fun getRecents(context: Context): List<RecentDocumentItem> {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val json = prefs.getString(KEY_LIST, null) ?: return emptyList()
        return try {
            val arr = JSONArray(json)
            val list = mutableListOf<RecentDocumentItem>()
            for (i in 0 until arr.length()) {
                val obj = arr.getJSONObject(i)
                list.add(
                    RecentDocumentItem(
                        uriString = obj.getString("uri"),
                        name = obj.getString("name"),
                        timestamp = obj.getLong("timestamp")
                    )
                )
            }
            list
        } catch (_: Exception) {
            emptyList()
        }
    }

    fun addRecent(context: Context, uriString: String, name: String) {
        val current = getRecents(context).filterNot { it.uriString == uriString }.toMutableList()
        current.add(0, RecentDocumentItem(uriString, name, System.currentTimeMillis()))
        val trimmed = current.take(15)
        val arr = JSONArray()
        for (item in trimmed) {
            val obj = JSONObject()
            obj.put("uri", item.uriString)
            obj.put("name", item.name)
            obj.put("timestamp", item.timestamp)
            arr.put(obj)
        }
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_LIST, arr.toString())
            .apply()
    }

    fun removeRecent(context: Context, uriString: String) {
        RecentThumbnailManager.removeThumbnail(context, uriString)
        val current = getRecents(context).filterNot { it.uriString == uriString }
        val arr = JSONArray()
        for (item in current) {
            val obj = JSONObject()
            obj.put("uri", item.uriString)
            obj.put("name", item.name)
            obj.put("timestamp", item.timestamp)
            arr.put(obj)
        }
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_LIST, arr.toString())
            .apply()
    }

    fun clearRecents(context: Context) {
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .remove(KEY_LIST)
            .apply()
    }
}

enum class AppThemeMode {
    SYSTEM,
    LIGHT,
    DARK
}

object ThemePreferences {
    private const val PREFS_NAME = "pdfcraft_settings"
    private const val KEY_THEME = "app_theme_mode"

    fun getTheme(context: Context): AppThemeMode {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val modeStr = prefs.getString(KEY_THEME, AppThemeMode.SYSTEM.name) ?: AppThemeMode.SYSTEM.name
        return try {
            AppThemeMode.valueOf(modeStr)
        } catch (_: Exception) {
            AppThemeMode.SYSTEM
        }
    }

    fun setTheme(context: Context, mode: AppThemeMode) {
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_THEME, mode.name)
            .apply()
    }
}

class MainActivity : ComponentActivity() {
    private var currentDocument: PdfDocument? = null

    companion object {
        init {
            try {
                System.loadLibrary("pdfcraft_ffi")
            } catch (t: Throwable) {
                android.util.Log.w("PdfCraft", "System.loadLibrary fallback: ${t.message}")
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        setContent {
            var themeMode by remember { mutableStateOf(ThemePreferences.getTheme(this)) }
            val systemDark = isSystemInDarkTheme()
            val isDark = when (themeMode) {
                AppThemeMode.SYSTEM -> systemDark
                AppThemeMode.LIGHT -> false
                AppThemeMode.DARK -> true
            }

            MaterialTheme(
                colorScheme = if (isDark) darkColorScheme() else lightColorScheme()
            ) {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background
                ) {
                    AppRootScreen(
                        initialIntent = intent,
                        onDocumentChanged = { currentDocument = it },
                        themeMode = themeMode,
                        onThemeChanged = { newMode ->
                            themeMode = newMode
                            ThemePreferences.setTheme(this, newMode)
                        }
                    )
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        if (level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL) {
            currentDocument?.handleMemoryPressure(uniffi.pdfcraft_ffi.FfiMemoryPressureLevel.CRITICAL)
        } else if (level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW) {
            currentDocument?.handleMemoryPressure(uniffi.pdfcraft_ffi.FfiMemoryPressureLevel.MODERATE)
        } else {
            currentDocument?.trimMemory()
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        currentDocument?.close()
        currentDocument = null
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AppRootScreen(
    initialIntent: Intent?,
    onDocumentChanged: (PdfDocument?) -> Unit,
    themeMode: AppThemeMode = AppThemeMode.SYSTEM,
    onThemeChanged: (AppThemeMode) -> Unit = {}
) {
    val context = LocalContext.current
    val coroutineScope = rememberCoroutineScope()

    var activeDoc by remember { mutableStateOf<PdfDocument?>(null) }
    var currentUri by remember { mutableStateOf<Uri?>(null) }
    var isLoading by remember { mutableStateOf(false) }
    var errorMessage by remember { mutableStateOf<String?>(null) }
    var showAbout by remember { mutableStateOf(false) }

    var selectedHomeTab by remember { mutableIntStateOf(0) }
    var isFabExpanded by remember { mutableStateOf(false) }
    var filterMode by remember { mutableStateOf("All") }

    var passwordPromptUri by remember { mutableStateOf<Uri?>(null) }
    var passwordInput by remember { mutableStateOf("") }
    var homeSearchQuery by remember { mutableStateOf("") }

    var pendingToolAction by remember { mutableStateOf<String?>(null) }
    var activeInitialToolAction by remember { mutableStateOf<String?>(null) }

    var recentFiles by remember { mutableStateOf(RecentDocumentsManager.getRecents(context)) }

    fun refreshRecents() {
        recentFiles = RecentDocumentsManager.getRecents(context)
    }

    fun loadUri(uri: Uri, password: String? = null, action: String? = null) {
        isLoading = true
        errorMessage = null
        coroutineScope.launch {
            try {
                try {
                    context.contentResolver.takePersistableUriPermission(
                        uri,
                        Intent.FLAG_GRANT_READ_URI_PERMISSION
                    )
                } catch (_: Throwable) {}

                var fileName = uri.lastPathSegment ?: "document.pdf"
                try {
                    context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                        val nameIdx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        if (nameIdx >= 0 && cursor.moveToFirst()) {
                            cursor.getString(nameIdx)?.let { fileName = it }
                        }
                    }
                } catch (_: Throwable) {}

                val bytes = withContext(Dispatchers.IO) {
                    context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
                } ?: throw IllegalStateException("Could not read stream from URI: $uri")

                val doc = withContext(Dispatchers.Default) {
                    PdfDocument.openBytes(name = fileName, bytes = bytes, password = password)
                }

                withContext(Dispatchers.Default) {
                    try {
                        val thumb = doc.renderThumbnail(0, 160f)
                        if (thumb != null) {
                            RecentThumbnailManager.saveThumbnail(context, uri.toString(), thumb)
                        }
                    } catch (_: Exception) {}
                }

                activeDoc?.close()
                activeDoc = doc
                currentUri = uri
                activeInitialToolAction = action
                RecentDocumentsManager.addRecent(context, uri.toString(), fileName)
                refreshRecents()
                onDocumentChanged(doc)
                passwordPromptUri = null
                passwordInput = ""
            } catch (e: Throwable) {
                val msg = e.message ?: ""
                if (msg.contains("password", ignoreCase = true) || msg.contains("NeedsPassword", ignoreCase = true) || msg.contains("WrongPassword", ignoreCase = true)) {
                    passwordPromptUri = uri
                } else {
                    errorMessage = e.message ?: "Failed to open document"
                }
            } finally {
                isLoading = false
            }
        }
    }

    fun loadDemoDocument(action: String? = null) {
        isLoading = true
        errorMessage = null
        coroutineScope.launch {
            try {
                val demoPdf = """
                    %PDF-1.4
                    1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
                    2 0 obj << /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >> endobj
                    3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj
                    4 0 obj << /Length 318 >> stream
                    0.12 0.23 0.54 rg
                    36 690 540 66 re f
                    1 1 1 rg
                    BT /F1 22 Tf 54 718 Td (PdfCraft Mobile Viewer) Tj ET
                    0.2 0.2 0.2 rg
                    BT /F1 13 Tf 54 640 Td (Pure-Rust Engine: Multi-page Tiled Renderer Online) Tj ET
                    BT /F1 11 Tf 54 610 Td (Pinch to zoom, pan to navigate, double-tap to reset scale.) Tj ET
                    BT /F1 11 Tf 54 585 Td (Use the toolbar below for highlights, annotations, and tools.) Tj ET
                    0.8 0.8 0.8 rg
                    36 560 540 1 re f
                    endstream endobj
                    5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
                    6 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 7 0 R >> endobj
                    7 0 obj << /Length 212 >> stream
                    0.2 0.4 0.2 rg
                    36 690 540 66 re f
                    1 1 1 rg
                    BT /F1 22 Tf 54 718 Td (Page 2: Automation & Tools) Tj ET
                    0.2 0.2 0.2 rg
                    BT /F1 13 Tf 54 640 Td (141 headless tools accessible from the Command Palette.) Tj ET
                    endstream endobj
                    trailer << /Size 8 /Root 1 0 R >>
                    %%EOF
                """.trimIndent().toByteArray(Charsets.UTF_8)

                val demoFile = withContext(Dispatchers.IO) {
                    val f = File(context.filesDir, "pdfcraft_showcase.pdf")
                    f.writeBytes(demoPdf)
                    f
                }
                loadUri(Uri.fromFile(demoFile), action = action)
            } catch (e: Throwable) {
                errorMessage = e.message ?: "Failed to generate demo PDF"
            } finally {
                isLoading = false
            }
        }
    }

    fun createBlankDocument(action: String? = null) {
        isLoading = true
        errorMessage = null
        coroutineScope.launch {
            try {
                val fileName = "Untitled_${(System.currentTimeMillis() % 100000)}.pdf"
                val blankFile = withContext(Dispatchers.IO) {
                    val f = File(context.filesDir, fileName)
                    val doc = PdfDocument.createBlank(name = fileName)
                    val bytes = doc.saveBytes()
                    doc.close()
                    f.writeBytes(bytes)
                    f
                }
                loadUri(Uri.fromFile(blankFile), action = action)
            } catch (e: Throwable) {
                errorMessage = e.message ?: "Failed to create blank document"
            } finally {
                isLoading = false
            }
        }
    }

    LaunchedEffect(initialIntent) {
        if (initialIntent?.action == Intent.ACTION_VIEW) {
            initialIntent.data?.let { uri ->
                loadUri(uri)
            }
        }
    }

    val launcher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.OpenDocument()
    ) { uri: Uri? ->
        uri?.let {
            loadUri(it, action = pendingToolAction)
            pendingToolAction = null
        }
    }

    fun launchTool(action: String) {
        pendingToolAction = action
        launcher.launch(arrayOf("application/pdf"))
    }

    val photoPickerLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.GetMultipleContents()
    ) { uris: List<Uri> ->
        if (uris.isNotEmpty()) {
            isLoading = true
            errorMessage = null
            coroutineScope.launch {
                try {
                    val images = withContext(Dispatchers.IO) {
                        uris.mapNotNull { u ->
                            context.contentResolver.openInputStream(u)?.use { it.readBytes() }
                        }
                    }
                    if (images.isNotEmpty()) {
                        val doc = withContext(Dispatchers.Default) {
                            PdfDocument.createFromImages(
                                name = "scan_${System.currentTimeMillis()}.pdf",
                                images = images
                            )
                        }
                        activeDoc?.close()
                        activeDoc = doc
                        currentUri = null
                        onDocumentChanged(doc)
                    }
                } catch (e: Throwable) {
                    errorMessage = e.message ?: "Failed to convert images to PDF"
                } finally {
                    isLoading = false
                }
            }
        }
    }

    val saveAsLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.CreateDocument("application/pdf")
    ) { targetUri: Uri? ->
        val doc = activeDoc
        if (targetUri != null && doc != null) {
            coroutineScope.launch {
                try {
                    val bytes = withContext(Dispatchers.Default) { doc.saveBytes() }
                    withContext(Dispatchers.IO) {
                        context.contentResolver.openOutputStream(targetUri, "wt")?.use { it.write(bytes) }
                    }
                    currentUri = targetUri
                    RecentDocumentsManager.addRecent(context, targetUri.toString(), doc.name)
                    refreshRecents()
                    Toast.makeText(context, "Saved as new document", Toast.LENGTH_SHORT).show()
                } catch (e: Throwable) {
                    Toast.makeText(context, "Save As error: ${e.message}", Toast.LENGTH_LONG).show()
                }
            }
        }
    }

    fun handleSave() {
        val doc = activeDoc ?: return
        val uri = currentUri
        if (uri != null) {
            coroutineScope.launch {
                try {
                    val bytes = withContext(Dispatchers.Default) { doc.saveBytes() }
                    withContext(Dispatchers.IO) {
                        context.contentResolver.openOutputStream(uri, "wt")?.use { it.write(bytes) }
                    }
                    Toast.makeText(context, "Document saved", Toast.LENGTH_SHORT).show()
                } catch (e: Throwable) {
                    Toast.makeText(context, "Save error: ${e.message}", Toast.LENGTH_LONG).show()
                }
            }
        } else {
            saveAsLauncher.launch(doc.name)
        }
    }

    fun handleShare() {
        val doc = activeDoc ?: return
        coroutineScope.launch {
            try {
                val bytes = withContext(Dispatchers.Default) { doc.saveBytes() }
                val sharedFile = withContext(Dispatchers.IO) {
                    val dir = File(context.cacheDir, "shared_pdfs").apply { mkdirs() }
                    val f = File(dir, doc.name)
                    f.writeBytes(bytes)
                    f
                }
                val contentUri = FileProvider.getUriForFile(
                    context,
                    "${context.packageName}.fileprovider",
                    sharedFile
                )
                val shareIntent = Intent(Intent.ACTION_SEND).apply {
                    type = "application/pdf"
                    putExtra(Intent.EXTRA_STREAM, contentUri)
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
                context.startActivity(Intent.createChooser(shareIntent, "Share PDF Document"))
            } catch (e: Throwable) {
                Toast.makeText(context, "Share error: ${e.message}", Toast.LENGTH_LONG).show()
            }
        }
    }

    val doc = activeDoc
    if (doc != null) {
        PdfView(
            document = doc,
            initialPageIndex = 0,
            initialAction = activeInitialToolAction,
            onClose = {
                doc.close()
                activeDoc = null
                currentUri = null
                activeInitialToolAction = null
                onDocumentChanged(null)
            },
            onSave = { handleSave() },
            onSaveAs = { saveAsLauncher.launch(doc.name) },
            onShare = { handleShare() }
        )
    } else {
        // Acrobat-Grade Material 3 Home Dashboard & Navigation Suite
        Scaffold(
            topBar = {
                TopAppBar(
                    title = {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Surface(
                                shape = RoundedCornerShape(8.dp),
                                color = MaterialTheme.colorScheme.primaryContainer,
                                modifier = Modifier.size(32.dp)
                            ) {
                                Box(contentAlignment = Alignment.Center) {
                                    Icon(
                                        imageVector = Icons.Outlined.PictureAsPdf,
                                        contentDescription = null,
                                        tint = MaterialTheme.colorScheme.onPrimaryContainer,
                                        modifier = Modifier.size(18.dp)
                                    )
                                }
                            }
                            Spacer(modifier = Modifier.width(10.dp))
                            Text(
                                text = when (selectedHomeTab) {
                                    0 -> "PdfCraft"
                                    1 -> "Files"
                                    2 -> "All Tools"
                                    else -> "Settings"
                                },
                                style = MaterialTheme.typography.titleLarge,
                                fontWeight = FontWeight.Bold
                            )
                        }
                    },
                    actions = {
                        if (selectedHomeTab == 0) {
                            TextButton(onClick = { showAbout = true }) {
                                Text("About")
                            }
                        }
                    }
                )
            },
            bottomBar = {
                NavigationBar {
                    NavigationBarItem(
                        selected = selectedHomeTab == 0,
                        onClick = { selectedHomeTab = 0 },
                        icon = {
                            Icon(
                                imageVector = if (selectedHomeTab == 0) Icons.Filled.Home else Icons.Outlined.Home,
                                contentDescription = "Home"
                            )
                        },
                        label = { Text("Home") }
                    )
                    NavigationBarItem(
                        selected = selectedHomeTab == 1,
                        onClick = { selectedHomeTab = 1 },
                        icon = {
                            Icon(
                                imageVector = if (selectedHomeTab == 1) Icons.Filled.Folder else Icons.Outlined.Folder,
                                contentDescription = "Files"
                            )
                        },
                        label = { Text("Files") }
                    )
                    NavigationBarItem(
                        selected = selectedHomeTab == 2,
                        onClick = { selectedHomeTab = 2 },
                        icon = {
                            Icon(
                                imageVector = if (selectedHomeTab == 2) Icons.Filled.Handyman else Icons.Outlined.Handyman,
                                contentDescription = "Tools"
                            )
                        },
                        label = { Text("Tools") }
                    )
                    NavigationBarItem(
                        selected = selectedHomeTab == 3,
                        onClick = { selectedHomeTab = 3 },
                        icon = {
                            Icon(
                                imageVector = if (selectedHomeTab == 3) Icons.Filled.Settings else Icons.Outlined.Settings,
                                contentDescription = "Settings"
                            )
                        },
                        label = { Text("Settings") }
                    )
                }
            },
            floatingActionButton = {
                if (selectedHomeTab == 0 || selectedHomeTab == 1) {
                    Column(horizontalAlignment = Alignment.End) {
                        if (isFabExpanded) {
                            Surface(
                                shape = RoundedCornerShape(16.dp),
                                color = MaterialTheme.colorScheme.surfaceVariant,
                                tonalElevation = 6.dp,
                                modifier = Modifier.padding(bottom = 12.dp)
                            ) {
                                Column(
                                    modifier = Modifier.padding(12.dp),
                                    verticalArrangement = Arrangement.spacedBy(6.dp)
                                ) {
                                    TextButton(
                                        onClick = {
                                            isFabExpanded = false
                                            photoPickerLauncher.launch("image/*")
                                        }
                                    ) {
                                        Icon(Icons.Outlined.AddPhotoAlternate, contentDescription = null, modifier = Modifier.size(18.dp))
                                        Spacer(modifier = Modifier.width(8.dp))
                                        Text("Photos to PDF")
                                    }
                                    TextButton(
                                        onClick = {
                                            isFabExpanded = false
                                            createBlankDocument()
                                        }
                                    ) {
                                        Icon(Icons.AutoMirrored.Outlined.NoteAdd, contentDescription = null, modifier = Modifier.size(18.dp))
                                        Spacer(modifier = Modifier.width(8.dp))
                                        Text("New Blank PDF")
                                    }
                                    TextButton(
                                        onClick = {
                                            isFabExpanded = false
                                            launcher.launch(arrayOf("application/pdf"))
                                        }
                                    ) {
                                        Icon(Icons.Outlined.FolderOpen, contentDescription = null, modifier = Modifier.size(18.dp))
                                        Spacer(modifier = Modifier.width(8.dp))
                                        Text("Open PDF File")
                                    }
                                }
                            }
                        }
                        ExtendedFloatingActionButton(
                            onClick = { isFabExpanded = !isFabExpanded },
                            icon = {
                                Icon(
                                    imageVector = if (isFabExpanded) Icons.Filled.Close else Icons.Filled.Add,
                                    contentDescription = null
                                )
                            },
                            text = { Text(if (isFabExpanded) "Close" else "Create") }
                        )
                    }
                }
            }
        ) { paddingValues ->
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(paddingValues)
            ) {
                when (selectedHomeTab) {
                    0 -> {
                        // HOME TAB
                        Column(
                            modifier = Modifier
                                .fillMaxSize()
                                .padding(horizontal = 16.dp)
                        ) {
                            // Quick Action Cards
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 12.dp),
                                horizontalArrangement = Arrangement.spacedBy(8.dp)
                            ) {
                                Surface(
                                    shape = RoundedCornerShape(12.dp),
                                    color = MaterialTheme.colorScheme.primaryContainer,
                                    modifier = Modifier
                                        .weight(1f)
                                        .height(90.dp)
                                        .clickable { launcher.launch(arrayOf("application/pdf")) }
                                 ) {
                                     Column(
                                         modifier = Modifier
                                             .fillMaxSize()
                                             .padding(10.dp),
                                         verticalArrangement = Arrangement.SpaceBetween
                                     ) {
                                         Icon(
                                             imageVector = Icons.Outlined.FolderOpen,
                                             contentDescription = null,
                                             tint = MaterialTheme.colorScheme.onPrimaryContainer,
                                             modifier = Modifier.size(24.dp)
                                         )
                                         Text(
                                             text = "Open File",
                                             style = MaterialTheme.typography.labelMedium,
                                             fontWeight = FontWeight.SemiBold,
                                             color = MaterialTheme.colorScheme.onPrimaryContainer
                                         )
                                     }
                                 }

                                Surface(
                                    shape = RoundedCornerShape(12.dp),
                                    color = MaterialTheme.colorScheme.secondaryContainer,
                                    modifier = Modifier
                                        .weight(1f)
                                        .height(90.dp)
                                        .clickable { photoPickerLauncher.launch("image/*") }
                                 ) {
                                     Column(
                                         modifier = Modifier
                                             .fillMaxSize()
                                             .padding(10.dp),
                                         verticalArrangement = Arrangement.SpaceBetween
                                     ) {
                                         Icon(
                                             imageVector = Icons.Outlined.PhotoCamera,
                                             contentDescription = null,
                                             tint = MaterialTheme.colorScheme.onSecondaryContainer,
                                             modifier = Modifier.size(24.dp)
                                         )
                                         Text(
                                             text = "Scan / Photos",
                                             style = MaterialTheme.typography.labelMedium,
                                             fontWeight = FontWeight.SemiBold,
                                             color = MaterialTheme.colorScheme.onSecondaryContainer
                                         )
                                     }
                                 }

                                Surface(
                                    shape = RoundedCornerShape(12.dp),
                                    color = MaterialTheme.colorScheme.tertiaryContainer,
                                    modifier = Modifier
                                        .weight(1f)
                                        .height(90.dp)
                                        .clickable { createBlankDocument() }
                                 ) {
                                     Column(
                                         modifier = Modifier
                                             .fillMaxSize()
                                             .padding(10.dp),
                                         verticalArrangement = Arrangement.SpaceBetween
                                     ) {
                                         Icon(
                                             imageVector = Icons.AutoMirrored.Outlined.NoteAdd,
                                             contentDescription = null,
                                             tint = MaterialTheme.colorScheme.onTertiaryContainer,
                                             modifier = Modifier.size(24.dp)
                                         )
                                         Text(
                                             text = "Blank PDF",
                                             style = MaterialTheme.typography.labelMedium,
                                             fontWeight = FontWeight.SemiBold,
                                             color = MaterialTheme.colorScheme.onTertiaryContainer
                                         )
                                     }
                                 }

                                Surface(
                                    shape = RoundedCornerShape(12.dp),
                                    color = MaterialTheme.colorScheme.surfaceVariant,
                                    modifier = Modifier
                                        .weight(1f)
                                        .height(90.dp)
                                        .clickable { loadDemoDocument() }
                                 ) {
                                     Column(
                                         modifier = Modifier
                                             .fillMaxSize()
                                             .padding(10.dp),
                                         verticalArrangement = Arrangement.SpaceBetween
                                     ) {
                                         Icon(
                                             imageVector = Icons.Outlined.AutoAwesome,
                                             contentDescription = null,
                                             tint = MaterialTheme.colorScheme.onSurfaceVariant,
                                             modifier = Modifier.size(24.dp)
                                         )
                                         Text(
                                             text = "Sample",
                                             style = MaterialTheme.typography.labelMedium,
                                             fontWeight = FontWeight.SemiBold,
                                             color = MaterialTheme.colorScheme.onSurfaceVariant
                                         )
                                     }
                                 }
                            }

                            // Error Banner
                            errorMessage?.let { err ->
                                Surface(
                                    shape = RoundedCornerShape(8.dp),
                                    color = MaterialTheme.colorScheme.errorContainer,
                                    modifier = Modifier.fillMaxWidth()
                                ) {
                                    Text(
                                        text = err,
                                        color = MaterialTheme.colorScheme.onErrorContainer,
                                        style = MaterialTheme.typography.bodySmall,
                                        modifier = Modifier.padding(12.dp),
                                        textAlign = TextAlign.Center
                                    )
                                }
                                Spacer(modifier = Modifier.height(8.dp))
                            }

                            if (isLoading) {
                                Box(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(24.dp),
                                    contentAlignment = Alignment.Center
                                ) {
                                    CircularProgressIndicator(modifier = Modifier.size(36.dp))
                                }
                            }

                            // Search Bar for Recents
                            OutlinedTextField(
                                value = homeSearchQuery,
                                onValueChange = { homeSearchQuery = it },
                                placeholder = { Text("Search files...") },
                                singleLine = true,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 4.dp)
                            )

                            // Filter Chips Row
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 6.dp),
                                horizontalArrangement = Arrangement.spacedBy(8.dp)
                            ) {
                                listOf("All", "Recent", "Favorites").forEach { filter ->
                                    FilterChip(
                                        selected = filterMode == filter,
                                        onClick = { filterMode = filter },
                                        label = { Text(filter) }
                                    )
                                }
                            }

                            // Recent Documents Header
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(top = 8.dp, bottom = 4.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Text(
                                    text = "Recent Files",
                                    style = MaterialTheme.typography.titleMedium,
                                    fontWeight = FontWeight.Bold
                                )
                                if (recentFiles.isNotEmpty()) {
                                    TextButton(
                                        onClick = {
                                            RecentDocumentsManager.clearRecents(context)
                                            refreshRecents()
                                        }
                                    ) {
                                        Text("Clear All")
                                    }
                                }
                            }

                            val filteredRecents = if (homeSearchQuery.isBlank()) {
                                recentFiles
                            } else {
                                recentFiles.filter { it.name.contains(homeSearchQuery, ignoreCase = true) }
                            }

                            if (filteredRecents.isEmpty()) {
                                Box(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .weight(1f),
                                    contentAlignment = Alignment.Center
                                ) {
                                    Text(
                                        text = if (homeSearchQuery.isBlank()) "No recent documents.\nTap '+ Create' or 'Open File' above to start reading." else "No matching files found.",
                                        style = MaterialTheme.typography.bodyMedium,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        textAlign = TextAlign.Center
                                    )
                                }
                            } else {
                                val dateFormat = SimpleDateFormat("MMM d, yyyy • HH:mm", Locale.getDefault())
                                LazyColumn(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .weight(1f),
                                    verticalArrangement = Arrangement.spacedBy(8.dp)
                                ) {
                                    items(filteredRecents) { recent ->
                                        Surface(
                                            shape = RoundedCornerShape(10.dp),
                                            color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f),
                                            modifier = Modifier
                                                .fillMaxWidth()
                                                .clickable { loadUri(Uri.parse(recent.uriString)) }
                                        ) {
                                            Row(
                                                modifier = Modifier
                                                    .fillMaxWidth()
                                                    .padding(12.dp),
                                                verticalAlignment = Alignment.CenterVertically
                                            ) {
                                                val thumbBitmap = remember(recent.uriString) {
                                                    RecentThumbnailManager.getThumbnail(context, recent.uriString)
                                                }
                                                if (thumbBitmap != null) {
                                                    Image(
                                                        bitmap = thumbBitmap.asImageBitmap(),
                                                        contentDescription = recent.name,
                                                        modifier = Modifier
                                                            .size(40.dp)
                                                            .clip(RoundedCornerShape(6.dp))
                                                            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(6.dp)),
                                                        contentScale = ContentScale.Crop
                                                    )
                                                } else {
                                                    Surface(
                                                        shape = RoundedCornerShape(6.dp),
                                                        color = MaterialTheme.colorScheme.primaryContainer,
                                                        modifier = Modifier.size(40.dp)
                                                    ) {
                                                        Box(contentAlignment = Alignment.Center) {
                                                            Text(
                                                                text = "PDF",
                                                                style = MaterialTheme.typography.labelSmall,
                                                                fontWeight = FontWeight.Bold,
                                                                color = MaterialTheme.colorScheme.onPrimaryContainer
                                                            )
                                                        }
                                                    }
                                                }

                                                Spacer(modifier = Modifier.width(12.dp))

                                                Column(modifier = Modifier.weight(1f)) {
                                                    Text(
                                                        text = recent.name,
                                                        style = MaterialTheme.typography.bodyMedium,
                                                        fontWeight = FontWeight.SemiBold,
                                                        maxLines = 1,
                                                        overflow = TextOverflow.Ellipsis
                                                    )
                                                    Text(
                                                        text = dateFormat.format(Date(recent.timestamp)),
                                                        style = MaterialTheme.typography.labelSmall,
                                                        color = MaterialTheme.colorScheme.onSurfaceVariant
                                                    )
                                                }

                                                IconButton(
                                                    onClick = {
                                                        RecentDocumentsManager.removeRecent(context, recent.uriString)
                                                        refreshRecents()
                                                    }
                                                ) {
                                                    Icon(
                                                        imageVector = Icons.Outlined.Close,
                                                        contentDescription = "Remove",
                                                        modifier = Modifier.size(16.dp),
                                                        tint = MaterialTheme.colorScheme.onSurfaceVariant
                                                    )
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    1 -> {
                        // FILES TAB
                        Column(
                            modifier = Modifier
                                .fillMaxSize()
                                .padding(16.dp)
                        ) {
                            Text(
                                text = "Storage Locations",
                                style = MaterialTheme.typography.titleMedium,
                                fontWeight = FontWeight.Bold
                            )
                            Spacer(modifier = Modifier.height(12.dp))

                            Surface(
                                shape = RoundedCornerShape(12.dp),
                                color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.6f),
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable { launcher.launch(arrayOf("application/pdf")) }
                            ) {
                                Row(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(16.dp),
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Icon(
                                        imageVector = Icons.Outlined.Folder,
                                        contentDescription = null,
                                        tint = MaterialTheme.colorScheme.primary,
                                        modifier = Modifier.size(32.dp)
                                    )
                                    Spacer(modifier = Modifier.width(16.dp))
                                    Column {
                                        Text("Browse Device Files", style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.SemiBold)
                                        Text("Choose from Downloads, Drive, SD Card", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                    }
                                }
                            }

                            Spacer(modifier = Modifier.height(12.dp))

                            Surface(
                                shape = RoundedCornerShape(12.dp),
                                color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.6f),
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        val downloadDir = android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS)
                                        val pdfFiles = downloadDir.listFiles { f -> f.extension.equals("pdf", ignoreCase = true) }
                                        if (!pdfFiles.isNullOrEmpty()) {
                                            loadUri(Uri.fromFile(pdfFiles.first()))
                                        } else {
                                            launcher.launch(arrayOf("application/pdf"))
                                        }
                                    }
                            ) {
                                Row(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(16.dp),
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    Icon(
                                        imageVector = Icons.Outlined.Download,
                                        contentDescription = null,
                                        tint = MaterialTheme.colorScheme.primary,
                                        modifier = Modifier.size(32.dp)
                                    )
                                    Spacer(modifier = Modifier.width(16.dp))
                                    Column {
                                        Text("Downloads Folder", style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.SemiBold)
                                        Text("Quick access to exported and downloaded PDFs", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                    }
                                }
                            }
                        }
                    }
                    2 -> {
                        // TOOLS TAB (Comprehensive PDF Toolkit)
                        LazyColumn(
                            modifier = Modifier
                                .fillMaxSize()
                                .padding(horizontal = 16.dp),
                            verticalArrangement = Arrangement.spacedBy(16.dp)
                        ) {
                            item {
                                Spacer(modifier = Modifier.height(4.dp))
                                Text("Convert & Export", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                                Spacer(modifier = Modifier.height(8.dp))
                                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.Description, title = "Export Word", desc = "Export to .docx") {
                                        launchTool("export_word")
                                    }
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.Slideshow, title = "PowerPoint", desc = "Export to .pptx") {
                                        launchTool("export_pptx")
                                    }
                                }
                                Spacer(modifier = Modifier.height(8.dp))
                                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.Image, title = "Export Images", desc = "PNG, JPEG, TIFF") {
                                        launchTool("export_images")
                                    }
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.Verified, title = "Convert PDF/A", desc = "Archival ISO 19005") {
                                        launchTool("convert_pdfa")
                                    }
                                }
                            }

                            item {
                                Text("Edit & Content", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                                Spacer(modifier = Modifier.height(8.dp))
                                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.Draw, title = "Draw & Markup", desc = "Pen, highlight, ink") {
                                        launchTool("draw")
                                    }
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.TextFields, title = "Add Text", desc = "Insert page text") {
                                        launchTool("add_text")
                                    }
                                }
                                Spacer(modifier = Modifier.height(8.dp))
                                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.AddPhotoAlternate, title = "Insert Image", desc = "Embed pictures") {
                                        launchTool("insert_image")
                                    }
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.VisibilityOff, title = "Redact", desc = "Permanent sanitize") {
                                        launchTool("redact")
                                    }
                                }
                                Spacer(modifier = Modifier.height(8.dp))
                                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.AutoMirrored.Outlined.BrandingWatermark, title = "Watermarks", desc = "Headers & Footers") {
                                        launchTool("watermark")
                                    }
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.AutoMirrored.Outlined.FormatListBulleted, title = "TOC / Outline", desc = "Bookmarks & outline") {
                                        launchTool("toc")
                                    }
                                }
                            }

                            item {
                                Text("Organize & Security", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                                Spacer(modifier = Modifier.height(8.dp))
                                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.GridView, title = "Organize Pages", desc = "Rotate, reorder, delete") {
                                        launchTool("organize")
                                    }
                                    ToolCard(modifier = Modifier.weight(1f), icon = Icons.Outlined.Lock, title = "Protect PDF", desc = "AES-256 encryption") {
                                        launchTool("protect")
                                    }
                                }
                                Spacer(modifier = Modifier.height(16.dp))
                            }
                        }
                    }
                    3 -> {
                        // SETTINGS TAB
                        Column(
                            modifier = Modifier
                                .fillMaxSize()
                                .padding(16.dp)
                                .verticalScroll(rememberScrollState())
                        ) {
                            Text("Appearance & Theme", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                            Spacer(modifier = Modifier.height(12.dp))
                            Surface(
                                shape = RoundedCornerShape(12.dp),
                                color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f),
                                modifier = Modifier.fillMaxWidth()
                            ) {
                                Column(modifier = Modifier.padding(16.dp)) {
                                    Text(
                                        text = "App Theme",
                                        style = MaterialTheme.typography.titleSmall,
                                        fontWeight = FontWeight.SemiBold
                                    )
                                    Text(
                                        text = "Choose light, dark, or follow device system theme",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant
                                    )
                                    Spacer(modifier = Modifier.height(12.dp))
                                    Row(
                                        modifier = Modifier.fillMaxWidth(),
                                        horizontalArrangement = Arrangement.spacedBy(8.dp)
                                    ) {
                                        listOf(
                                            Triple(AppThemeMode.SYSTEM, "System", Icons.Outlined.BrightnessAuto),
                                            Triple(AppThemeMode.LIGHT, "Light", Icons.Outlined.LightMode),
                                            Triple(AppThemeMode.DARK, "Dark", Icons.Outlined.DarkMode)
                                        ).forEach { (mode, label, icon) ->
                                            val isSelected = themeMode == mode
                                            OutlinedCard(
                                                onClick = { onThemeChanged(mode) },
                                                colors = CardDefaults.outlinedCardColors(
                                                    containerColor = if (isSelected) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surface
                                                ),
                                                modifier = Modifier.weight(1f)
                                            ) {
                                                Column(
                                                    modifier = Modifier
                                                        .fillMaxWidth()
                                                        .padding(vertical = 12.dp, horizontal = 8.dp),
                                                    horizontalAlignment = Alignment.CenterHorizontally,
                                                    verticalArrangement = Arrangement.spacedBy(6.dp)
                                                ) {
                                                    Icon(
                                                        imageVector = icon,
                                                        contentDescription = label,
                                                        tint = if (isSelected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                                                        modifier = Modifier.size(22.dp)
                                                    )
                                                    Text(
                                                        text = label,
                                                        style = MaterialTheme.typography.labelMedium,
                                                        fontWeight = if (isSelected) FontWeight.Bold else FontWeight.Normal,
                                                        color = if (isSelected) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface
                                                    )
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            Spacer(modifier = Modifier.height(20.dp))

                            Text("About PdfCraft", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                            Spacer(modifier = Modifier.height(12.dp))
                            Surface(
                                shape = RoundedCornerShape(12.dp),
                                color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f),
                                modifier = Modifier.fillMaxWidth()
                            ) {
                                Column(modifier = Modifier.padding(16.dp)) {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Icon(
                                            imageVector = Icons.Outlined.PictureAsPdf,
                                            contentDescription = null,
                                            tint = MaterialTheme.colorScheme.primary,
                                            modifier = Modifier.size(28.dp)
                                        )
                                        Spacer(modifier = Modifier.width(10.dp))
                                        Column {
                                            Text("PdfCraft Mobile", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                                            Text("Version 0.7.0 • Pure-Rust Engine", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                        }
                                    }
                                    Spacer(modifier = Modifier.height(12.dp))
                                    Text(
                                        text = "High-fidelity PDF reader, editor, and multi-format conversion suite powered by Rust and Android Material 3.",
                                        style = MaterialTheme.typography.bodyMedium
                                    )
                                    Spacer(modifier = Modifier.height(16.dp))
                                    Button(onClick = { showAbout = true }) {
                                        Icon(Icons.Outlined.Info, contentDescription = null, modifier = Modifier.size(18.dp))
                                        Spacer(modifier = Modifier.width(8.dp))
                                        Text("View Licenses & Attributions")
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Password Prompt Dialog
    passwordPromptUri?.let { uri ->
        AlertDialog(
            onDismissRequest = { passwordPromptUri = null },
            title = { Text("Password Required") },
            text = {
                Column {
                    Text(
                        text = "This PDF document is encrypted. Enter the password to open it.",
                        style = MaterialTheme.typography.bodySmall
                    )
                    Spacer(modifier = Modifier.height(12.dp))
                    OutlinedTextField(
                        value = passwordInput,
                        onValueChange = { passwordInput = it },
                        label = { Text("Password") },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        modifier = Modifier.fillMaxWidth()
                    )
                }
            },
            confirmButton = {
                Button(onClick = { loadUri(uri, password = passwordInput) }) {
                    Text("Open")
                }
            },
            dismissButton = {
                TextButton(onClick = { passwordPromptUri = null }) {
                    Text("Cancel")
                }
            }
        )
    }

    if (showAbout) {
        androidx.compose.ui.window.Dialog(onDismissRequest = { showAbout = false }) {
            PdfAboutView(onDismiss = { showAbout = false })
        }
    }
}

@Composable
fun ToolCard(
    icon: ImageVector,
    title: String,
    desc: String,
    modifier: Modifier = Modifier,
    iconTint: Color = MaterialTheme.colorScheme.primary,
    onClick: () -> Unit
) {
    Surface(
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.6f),
        modifier = modifier
            .height(84.dp)
            .clickable(onClick = onClick)
    ) {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(10.dp),
            verticalArrangement = Arrangement.SpaceBetween
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = icon,
                    contentDescription = null,
                    modifier = Modifier.size(20.dp),
                    tint = iconTint
                )
                Spacer(modifier = Modifier.width(8.dp))
                Text(
                    text = title,
                    style = MaterialTheme.typography.labelMedium,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis
                )
            }
            Text(
                text = desc,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis
            )
        }
    }
}
