package com.storytold.pdfcraft.kit

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

/**
 * Mobile PDF Conversion and Export dialog:
 * - Export Text (.txt)
 * - Export Images (PNG, JPEG, TIFF)
 * - Convert to PDF/A (PDF/A-2b, PDF/A-3b)
 * - Flatten annotations & form fields
 */
@Composable
fun PdfExportDialog(
    document: PdfDocument,
    onDismiss: () -> Unit,
    onExportText: (outPath: String) -> Unit,
    onExportImages: (outDir: String, format: String, dpi: Float) -> Unit,
    onConvertToPdfA: (level: String) -> Unit,
    onFlatten: (comments: Boolean, fields: Boolean) -> Unit,
    modifier: Modifier = Modifier
) {
    var selectedTab by remember { mutableStateOf(0) }
    val tabs = listOf("Text", "Images", "PDF/A", "Flatten")

    Surface(
        shape = RoundedCornerShape(16.dp),
        tonalElevation = 6.dp,
        modifier = modifier.fillMaxWidth()
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(24.dp)
                .verticalScroll(rememberScrollState())
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Text(
                    text = "Export & Convert",
                    style = MaterialTheme.typography.headlineSmall,
                    fontWeight = FontWeight.Bold
                )
                TextButton(onClick = onDismiss) {
                    Text("Cancel")
                }
            }

            Spacer(modifier = Modifier.height(16.dp))

            TabRow(selectedTabIndex = selectedTab) {
                tabs.forEachIndexed { index, title ->
                    Tab(
                        selected = selectedTab == index,
                        onClick = { selectedTab = index },
                        text = { Text(title) }
                    )
                }
            }

            Spacer(modifier = Modifier.height(20.dp))

            when (selectedTab) {
                0 -> {
                    // Export Text
                    Text(
                        text = "Export entire document or reading-order text to a plain text file.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(16.dp))
                    Button(
                        onClick = {
                            val targetFile = java.io.File(android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS), "${document.name.removeSuffix(".pdf")}.txt")
                            onExportText(targetFile.absolutePath)
                        },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Export to .txt File")
                    }
                }
                1 -> {
                    // Export Images
                    var format by remember { mutableStateOf("png") }
                    var dpi by remember { mutableFloatStateOf(150f) }

                    Text(
                        text = "Export each page as high-resolution image files.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))

                    Text("Format", style = MaterialTheme.typography.labelLarge)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        listOf("png", "jpeg", "tiff").forEach { fmt ->
                            FilterChip(
                                selected = format == fmt,
                                onClick = { format = fmt },
                                label = { Text(fmt.uppercase()) }
                            )
                        }
                    }

                    Spacer(modifier = Modifier.height(12.dp))
                    Text("Resolution: ${dpi.toInt()} DPI", style = MaterialTheme.typography.labelLarge)
                    Slider(
                        value = dpi,
                        onValueChange = { dpi = it },
                        valueRange = 72f..300f,
                        steps = 2
                    )

                    Spacer(modifier = Modifier.height(16.dp))
                    Button(
                        onClick = {
                            val targetDir = java.io.File(android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS), "${document.name.removeSuffix(".pdf")}_images")
                            targetDir.mkdirs()
                            onExportImages(targetDir.absolutePath, format, dpi)
                        },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Export Pages as Images")
                    }
                }
                2 -> {
                    // Convert to PDF/A
                    var pdfaLevel by remember { mutableStateOf("pdfa2b") }
                    Text(
                        text = "Convert document to ISO 19005 archival standards (PDF/A).",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        listOf("pdfa2b" to "PDF/A-2b", "pdfa3b" to "PDF/A-3b").forEach { (lvl, label) ->
                            FilterChip(
                                selected = pdfaLevel == lvl,
                                onClick = { pdfaLevel = lvl },
                                label = { Text(label) }
                            )
                        }
                    }

                    Spacer(modifier = Modifier.height(16.dp))
                    Button(
                        onClick = { onConvertToPdfA(pdfaLevel) },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Convert to ${pdfaLevel.uppercase()}")
                    }
                }
                3 -> {
                    // Flatten
                    var flattenComments by remember { mutableStateOf(true) }
                    var flattenFields by remember { mutableStateOf(true) }

                    Text(
                        text = "Bake interactive comments, drawings, and form fields directly into page content permanently.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(12.dp))

                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(
                            checked = flattenComments,
                            onCheckedChange = { flattenComments = it }
                        )
                        Text("Flatten Annotations and Comments", style = MaterialTheme.typography.bodyMedium)
                    }

                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(
                            checked = flattenFields,
                            onCheckedChange = { flattenFields = it }
                        )
                        Text("Flatten Form Fields", style = MaterialTheme.typography.bodyMedium)
                    }

                    Spacer(modifier = Modifier.height(16.dp))
                    Button(
                        onClick = { onFlatten(flattenComments, flattenFields) },
                        modifier = Modifier.fillMaxWidth(),
                        colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error)
                    ) {
                        Text("Flatten Document")
                    }
                }
            }
        }
    }
}
