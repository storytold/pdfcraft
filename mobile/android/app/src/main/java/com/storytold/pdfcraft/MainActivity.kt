package com.storytold.pdfcraft

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import com.storytold.pdfcraft.kit.PdfDocument
import com.storytold.pdfcraft.kit.PdfPageView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

class MainActivity : ComponentActivity() {
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
                Surface(modifier = Modifier.fillMaxSize()) {
                    ViewerScreen()
                }
            }
        }
    }
}

@Composable
fun ViewerScreen() {
    var statusText by remember { mutableStateOf("Initializing engine...") }
    var document by remember { mutableStateOf<PdfDocument?>(null) }

    LaunchedEffect(Unit) {
        withContext(Dispatchers.Default) {
            try {
                val pdfString = """
                    %PDF-1.4
                    1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
                    2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
                    3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj
                    4 0 obj << /Length 262 >> stream
                    0.12 0.23 0.54 rg
                    36 700 540 56 re f
                    1 1 1 rg
                    BT /F1 24 Tf 54 720 Td (PdfCraft Mobile Viewer) Tj ET
                    0.2 0.2 0.2 rg
                    BT /F1 14 Tf 54 650 Td (Engine Online: Phase 1 Tiled Renderer Active) Tj ET
                    BT /F1 12 Tf 54 620 Td (Pinch to zoom, pan to navigate, double-tap to reset.) Tj ET
                    0.8 0.8 0.8 rg
                    36 590 540 1 re f
                    endstream endobj
                    5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
                    trailer << /Size 6 /Root 1 0 R >>
                    %%EOF
                """.trimIndent()
                val bytes = pdfString.toByteArray(Charsets.UTF_8)
                val doc = PdfDocument.openBytes(name = "viewer_smoke.pdf", bytes = bytes)
                document = doc
                statusText = "Document loaded (${doc.pageCount} page) - pinch, pan, or double-tap to zoom"
            } catch (e: Throwable) {
                statusText = "Error: ${e.message}"
            }
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally
    ) {
        Text(text = "PdfCraft Mobile Viewer", style = MaterialTheme.typography.headlineSmall)
        Spacer(modifier = Modifier.height(4.dp))
        Text(text = statusText, style = MaterialTheme.typography.bodyMedium)
        Spacer(modifier = Modifier.height(12.dp))

        val currentDoc = document
        if (currentDoc != null) {
            AndroidView(
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f),
                factory = { ctx ->
                    PdfPageView(ctx).apply {
                        setDocument(currentDoc, 0)
                    }
                },
                update = { view ->
                    if (view.document != currentDoc) {
                        view.setDocument(currentDoc, 0)
                    }
                }
            )
        } else {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .weight(1f),
                contentAlignment = Alignment.Center
            ) {
                CircularProgressIndicator()
            }
        }
    }
}
