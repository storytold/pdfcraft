package ai.storyteller.pdfcraft

import ai.storyteller.pdfcraft.ui.PdfScreen
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels

/** Milestone 1: pick a PDF, view its pages, navigate. No editing yet. */
class MainActivity : ComponentActivity() {
    private val viewModel: PdfViewModel by viewModels()

    private val openDocument =
        registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            viewModel.open(uri?.let { contentResolver.openInputStream(it)?.use { stream -> stream.readBytes() } })
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            PdfScreen(viewModel = viewModel, onPickDocument = { openDocument.launch(arrayOf("application/pdf")) })
        }
    }
}
