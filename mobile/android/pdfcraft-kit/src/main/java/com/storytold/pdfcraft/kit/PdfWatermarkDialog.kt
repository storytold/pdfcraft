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
 * Mobile Watermark & Header/Footer editing dialog.
 */
@Composable
fun PdfWatermarkDialog(
    onDismiss: () -> Unit,
    onAddWatermark: (text: String, opacity: Float, rotation: Double) -> Unit,
    onAddHeaderFooter: (headerLeft: String?, headerCenter: String?, headerRight: String?, footerLeft: String?, footerCenter: String?, footerRight: String?, shrink: Boolean) -> Unit,
    modifier: Modifier = Modifier
) {
    var selectedTab by remember { mutableStateOf(0) }
    val tabs = listOf("Watermark", "Header & Footer")

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
                    text = "Page Marks",
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
                    // Watermark
                    var watermarkText by remember { mutableStateOf("CONFIDENTIAL") }
                    var opacity by remember { mutableFloatStateOf(0.3f) }
                    var rotation by remember { mutableDoubleStateOf(45.0) }

                    OutlinedTextField(
                        value = watermarkText,
                        onValueChange = { watermarkText = it },
                        label = { Text("Watermark Text") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )

                    Spacer(modifier = Modifier.height(16.dp))
                    Text("Opacity: ${(opacity * 100).toInt()}%", style = MaterialTheme.typography.labelLarge)
                    Slider(
                        value = opacity,
                        onValueChange = { opacity = it },
                        valueRange = 0.05f..1.0f
                    )

                    Spacer(modifier = Modifier.height(12.dp))
                    Text("Rotation", style = MaterialTheme.typography.labelLarge)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        listOf(0.0 to "0° (Horizontal)", 45.0 to "45° (Diagonal)", 90.0 to "90° (Vertical)").forEach { (angle, label) ->
                            FilterChip(
                                selected = rotation == angle,
                                onClick = { rotation = angle },
                                label = { Text(label) }
                            )
                        }
                    }

                    Spacer(modifier = Modifier.height(20.dp))
                    Button(
                        onClick = {
                            onAddWatermark(watermarkText, opacity, rotation)
                            onDismiss()
                        },
                        enabled = watermarkText.isNotBlank(),
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Add Watermark")
                    }
                }
                1 -> {
                    // Header & Footer
                    var headerLeft by remember { mutableStateOf("") }
                    var headerCenter by remember { mutableStateOf("") }
                    var headerRight by remember { mutableStateOf("") }
                    var footerLeft by remember { mutableStateOf("") }
                    var footerCenter by remember { mutableStateOf("<<Page 1 of n>>") }
                    var footerRight by remember { mutableStateOf("<<m/d/yyyy>>") }
                    var shrinkContent by remember { mutableStateOf(false) }

                    Text("Header", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = headerLeft,
                        onValueChange = { headerLeft = it },
                        label = { Text("Header Left") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = headerCenter,
                        onValueChange = { headerCenter = it },
                        label = { Text("Header Center") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = headerRight,
                        onValueChange = { headerRight = it },
                        label = { Text("Header Right") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )

                    Spacer(modifier = Modifier.height(16.dp))
                    Text("Footer", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = footerLeft,
                        onValueChange = { footerLeft = it },
                        label = { Text("Footer Left") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = footerCenter,
                        onValueChange = { footerCenter = it },
                        label = { Text("Footer Center") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = footerRight,
                        onValueChange = { footerRight = it },
                        label = { Text("Footer Right") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth()
                    )

                    Spacer(modifier = Modifier.height(12.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(
                            checked = shrinkContent,
                            onCheckedChange = { shrinkContent = it }
                        )
                        Text("Shrink page content to avoid overlap", style = MaterialTheme.typography.bodyMedium)
                    }

                    Spacer(modifier = Modifier.height(20.dp))
                    Button(
                        onClick = {
                            onAddHeaderFooter(
                                headerLeft.ifBlank { null },
                                headerCenter.ifBlank { null },
                                headerRight.ifBlank { null },
                                footerLeft.ifBlank { null },
                                footerCenter.ifBlank { null },
                                footerRight.ifBlank { null },
                                shrinkContent
                            )
                            onDismiss()
                        },
                        modifier = Modifier.fillMaxWidth()
                    ) {
                        Text("Add Header & Footer")
                    }
                }
            }
        }
    }
}
