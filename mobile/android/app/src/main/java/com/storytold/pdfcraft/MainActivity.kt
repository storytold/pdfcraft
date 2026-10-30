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
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
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
        val trimmed = current.take(10)
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

    fun clearRecents(context: Context) {
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .remove(KEY_LIST)
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
            MaterialTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background
                ) {
                    AppRootScreen(
                        initialIntent = intent,
                        onDocumentChanged = { currentDocument = it }
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

@Composable
fun AppRootScreen(
    initialIntent: Intent?,
    onDocumentChanged: (PdfDocument?) -> Unit
) {
    val context = LocalContext.current
    val coroutineScope = rememberCoroutineScope()

    var activeDoc by remember { mutableStateOf<PdfDocument?>(null) }
    var currentUri by remember { mutableStateOf<Uri?>(null) }
    var isLoading by remember { mutableStateOf(false) }
    var errorMessage by remember { mutableStateOf<String?>(null) }
    var showAbout by remember { mutableStateOf(false) }
    var recentFiles by remember { mutableStateOf(RecentDocumentsManager.getRecents(context)) }

    fun refreshRecents() {
        recentFiles = RecentDocumentsManager.getRecents(context)
    }

    fun loadUri(uri: Uri) {
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

                var fileName = "document.pdf"
                context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                    val nameIdx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (nameIdx >= 0 && cursor.moveToFirst()) {
                        fileName = cursor.getString(nameIdx)
                    }
                }

                val bytes = withContext(Dispatchers.IO) {
                    context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
                } ?: throw IllegalStateException("Could not read stream from URI: $uri")

                val doc = withContext(Dispatchers.Default) {
                    PdfDocument.openBytes(name = fileName, bytes = bytes)
                }
                activeDoc?.close()
                activeDoc = doc
                currentUri = uri
                RecentDocumentsManager.addRecent(context, uri.toString(), fileName)
                refreshRecents()
                onDocumentChanged(doc)
            } catch (e: Throwable) {
                errorMessage = e.message ?: "Failed to open document"
            } finally {
                isLoading = false
            }
        }
    }

    fun loadDemoDocument() {
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

                val doc = withContext(Dispatchers.Default) {
                    PdfDocument.openBytes(name = "pdfcraft_showcase.pdf", bytes = demoPdf)
                }
                activeDoc?.close()
                activeDoc = doc
                currentUri = null
                onDocumentChanged(doc)
            } catch (e: Throwable) {
                errorMessage = e.message ?: "Failed to generate demo PDF"
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
        uri?.let { loadUri(it) }
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
            onClose = {
                doc.close()
                activeDoc = null
                currentUri = null
                onDocumentChanged(null)
            },
            onSave = { handleSave() },
            onSaveAs = { saveAsLauncher.launch(doc.name) },
            onShare = { handleShare() }
        )
    } else {
        // Welcome / Launcher Screen
        Box(
            modifier = Modifier.fillMaxSize(),
            contentAlignment = Alignment.Center
        ) {
            Column(
                modifier = Modifier
                    .fillMaxWidth(0.9f)
                    .padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center
            ) {
                Surface(
                    shape = RoundedCornerShape(24.dp),
                    color = MaterialTheme.colorScheme.primaryContainer,
                    modifier = Modifier.size(80.dp)
                ) {
                    Box(contentAlignment = Alignment.Center) {
                        Text(
                            text = "PDF",
                            style = MaterialTheme.typography.titleLarge,
                            fontWeight = FontWeight.Bold,
                            color = MaterialTheme.colorScheme.onPrimaryContainer
                        )
                    }
                }

                Spacer(modifier = Modifier.height(20.dp))

                Text(
                    text = "PdfCraft Mobile",
                    style = MaterialTheme.typography.headlineMedium,
                    fontWeight = FontWeight.Bold
                )

                Spacer(modifier = Modifier.height(6.dp))

                Text(
                    text = "High-performance, memory-efficient PDF engine with 100% on-device privacy.",
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Center,
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )

                Spacer(modifier = Modifier.height(20.dp))

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
                    Spacer(modifier = Modifier.height(16.dp))
                }

                if (isLoading) {
                    CircularProgressIndicator(modifier = Modifier.size(36.dp))
                    Spacer(modifier = Modifier.height(12.dp))
                    Text(
                        text = "Loading document...",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.secondary
                    )
                } else {
                    Button(
                        onClick = { launcher.launch(arrayOf("application/pdf")) },
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(52.dp)
                    ) {
                        Text("Open PDF Document", fontWeight = FontWeight.SemiBold)
                    }

                    Spacer(modifier = Modifier.height(10.dp))

                    OutlinedButton(
                        onClick = { loadDemoDocument() },
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(52.dp)
                    ) {
                        Text("Load Sample Showcase Document")
                    }

                    // Recent Documents Section
                    if (recentFiles.isNotEmpty()) {
                        Spacer(modifier = Modifier.height(20.dp))
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically
                        ) {
                            Text(
                                text = "Recent Documents",
                                style = MaterialTheme.typography.titleSmall,
                                fontWeight = FontWeight.SemiBold
                            )
                            TextButton(
                                onClick = {
                                    RecentDocumentsManager.clearRecents(context)
                                    refreshRecents()
                                },
                                contentPadding = PaddingValues(horizontal = 4.dp, vertical = 0.dp)
                            ) {
                                Text("Clear", style = MaterialTheme.typography.labelSmall)
                            }
                        }

                        Spacer(modifier = Modifier.height(6.dp))

                        val dateFormat = SimpleDateFormat("MMM d, HH:mm", Locale.getDefault())
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(max = 160.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp)
                        ) {
                            recentFiles.take(3).forEach { recent ->
                                Surface(
                                    shape = RoundedCornerShape(8.dp),
                                    color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.6f),
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .clickable {
                                            loadUri(Uri.parse(recent.uriString))
                                        }
                                ) {
                                    Row(
                                        modifier = Modifier
                                            .fillMaxWidth()
                                            .padding(horizontal = 12.dp, vertical = 8.dp),
                                        horizontalArrangement = Arrangement.SpaceBetween,
                                        verticalAlignment = Alignment.CenterVertically
                                    ) {
                                        Text(
                                            text = recent.name,
                                            style = MaterialTheme.typography.bodyMedium,
                                            fontWeight = FontWeight.Medium,
                                            maxLines = 1,
                                            overflow = TextOverflow.Ellipsis,
                                            modifier = Modifier.weight(1f)
                                        )
                                        Spacer(modifier = Modifier.width(8.dp))
                                        Text(
                                            text = dateFormat.format(Date(recent.timestamp)),
                                            style = MaterialTheme.typography.labelSmall,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant
                                        )
                                    }
                                }
                            }
                        }
                    }

                    Spacer(modifier = Modifier.height(16.dp))

                    TextButton(onClick = { showAbout = true }) {
                        Text("About & Open Source Credits")
                    }
                }
            }
        }
    }

    if (showAbout) {
        androidx.compose.ui.window.Dialog(onDismissRequest = { showAbout = false }) {
            PdfAboutView(onDismiss = { showAbout = false })
        }
    }
}
