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
 * - Export Office: Word (.docx), PowerPoint (.pptx), HTML (.html), RTF (.rtf)
 * - Export / Extract Images: Render pages (PNG, JPEG, TIFF) or extract embedded images
 * - Export Text (.txt)
 * - Convert to PDF/A (PDF/A-2b, PDF/A-3b)
 * - Flatten annotations & form fields
 */
@Composable
fun PdfExportDialog(
    document: PdfDocument,
    onDismiss: () -> Unit,
    onExportOffice: (formatExt: String, outPath: String) -> Unit,
    onExportText: (outPath: String) -> Unit,
    onExportImages: (outDir: String, format: String, dpi: Float) -> Unit,
    onExtractAllImages: (outDir: String) -> Unit,
    onConvertToPdfA: (level: String) -> Unit,
    onFlatten: (comments: Boolean, fields: Boolean) -> Unit,
    initialTab: Int = 0,
    initialOfficeFormat: String = "docx",
    modifier: Modifier = Modifier
) {
    var selectedTab by remember { mutableIntStateOf(initialTab) }
    val tabs = listOf("Office", "Images", "Text", "PDF/A", "Flatten")

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

            ScrollableTabRow(
                selectedTabIndex = selectedTab,
                edgePadding = 0.dp
            ) {
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
                    // Export to Office formats: Word, PowerPoint, HTML, RTF
                    var selectedFormat by remember { mutableStateOf(initialOfficeFormat) }
                    val formats = listOf(
                        "docx" to "Word Document (.docx)",
                        "pptx" to "PowerPoint (.pptx)",
                        "html" to "Web Page (.html)",
                        "rtf" to "Rich Text (.rtf)"
                    )

                    Text(
                        text = "Export PDF to editable Microsoft Office, PowerPoint, or web documents with reconstructed flow and formatting.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(16.dp))

                    Text("Target Format", style = MaterialTheme.typography.labelLarge)
                    Spacer(modifier = Modifier.height(8.dp))

                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        formats.forEach { (ext, label) ->
                            OutlinedCard(
                                onClick = { selectedFormat = ext },
                                colors = CardDefaults.outlinedCardColors(
                                    containerColor = if (selectedFormat == ext) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surface
                                ),
                                modifier = Modifier.fillMaxWidth()
                            ) {
                                Row(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .padding(12.dp),
                                    verticalAlignment = Alignment.CenterVertically
                                ) {
                                    RadioButton(
                                        selected = selectedFormat == ext,
                                        onClick = { selectedFormat = ext }
                                    )
                                    Spacer(modifier = Modifier.width(8.dp))
                                    Text(
                                        text = label,
                                        style = MaterialTheme.typography.bodyMedium,
                                        fontWeight = if (selectedFormat == ext) FontWeight.Bold else FontWeight.Normal
                                    )
                                }
                            }
                        }
                    }

                    Spacer(modifier = Modifier.height(20.dp))
                    Button(
                        onClick = {
                            val baseName = document.name.removeSuffix(".pdf")
                            val targetFile = java.io.File(
                                android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS),
                                "$baseName.$selectedFormat"
                            )
                            onExportOffice(selectedFormat, targetFile.absolutePath)
                        },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Export to ${selectedFormat.uppercase()}")
                    }
                }
                1 -> {
                    // Export / Extract Images
                    var imageMode by remember { mutableStateOf("render") }
                    var format by remember { mutableStateOf("png") }
                    var dpi by remember { mutableFloatStateOf(150f) }

                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(
                            selected = imageMode == "render",
                            onClick = { imageMode = "render" },
                            label = { Text("Render Pages") }
                        )
                        FilterChip(
                            selected = imageMode == "extract",
                            onClick = { imageMode = "extract" },
                            label = { Text("Extract All Images") }
                        )
                    }

                    Spacer(modifier = Modifier.height(12.dp))

                    if (imageMode == "render") {
                        Text(
                            text = "Render each page as high-resolution image files (PNG, JPEG, or TIFF).",
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
                                val targetDir = java.io.File(
                                    android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS),
                                    "${document.name.removeSuffix(".pdf")}_pages"
                                )
                                targetDir.mkdirs()
                                onExportImages(targetDir.absolutePath, format, dpi)
                            },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("Export Pages as ${format.uppercase()}")
                        }
                    } else {
                        Text(
                            text = "Extract all embedded raster images from within the PDF into an output directory without quality loss.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                        Spacer(modifier = Modifier.height(20.dp))
                        Button(
                            onClick = {
                                val targetDir = java.io.File(
                                    android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS),
                                    "${document.name.removeSuffix(".pdf")}_extracted_images"
                                )
                                targetDir.mkdirs()
                                onExtractAllImages(targetDir.absolutePath)
                            },
                            modifier = Modifier.fillMaxWidth()
                        ) {
                            Text("Extract All Embedded Images")
                        }
                    }
                }
                2 -> {
                    // Export Text
                    Text(
                        text = "Export entire document or reading-order text to a plain text file (.txt).",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.height(16.dp))
                    Button(
                        onClick = {
                            val targetFile = java.io.File(
                                android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS),
                                "${document.name.removeSuffix(".pdf")}.txt"
                            )
                            onExportText(targetFile.absolutePath)
                        },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Export to .txt File")
                    }
                }
                3 -> {
                    // Convert to PDF/A
                    var pdfaLevel by remember { mutableStateOf("pdfa2b") }
                    Text(
                        text = "Convert document to ISO 19005 archival standards (PDF/A). Automatically repairs missing output intents, metadata schemas, and device color spaces.",
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
                4 -> {
                    // Flatten
                    var flattenComments by remember { mutableStateOf(true) }
                    var flattenFields by remember { mutableStateOf(true) }

                    Text(
                        text = "Bake interactive comments, drawings, signatures, and form fields directly into page content permanently.",
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
