package com.storytold.pdfcraft.kit

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

/**
 * About & Open Source Licenses dialog (Phase 7 / MOB-7.6).
 */
@Composable
fun PdfAboutView(
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier
) {
    val scrollState = rememberScrollState()

    Surface(
        shape = RoundedCornerShape(16.dp),
        tonalElevation = 6.dp,
        modifier = modifier.fillMaxWidth()
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(24.dp)
                .verticalScroll(scrollState)
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Text(
                    text = "PdfCraft",
                    style = MaterialTheme.typography.headlineMedium,
                    fontWeight = FontWeight.Bold
                )
                TextButton(onClick = onDismiss) {
                    Text("Close")
                }
            }

            Text(
                text = "Version 0.7.0 (Mobile)",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.secondary
            )
            Text(
                text = "Pure-Rust PDF Engine across Desktop, Web & Mobile",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )

            Spacer(modifier = Modifier.height(16.dp))

            // Privacy & Security
            Text(
                text = "Privacy & Security",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold
            )
            Spacer(modifier = Modifier.height(8.dp))
            Surface(
                shape = RoundedCornerShape(8.dp),
                color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.5f),
                modifier = Modifier.fillMaxWidth()
            ) {
                Column(modifier = Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("• 100% On-Device Processing (Zero Cloud Dependency)", style = MaterialTheme.typography.bodyMedium)
                    Text("• Zero Telemetry, Tracking, or Analytics", style = MaterialTheme.typography.bodyMedium)
                    Text("• Atomic & Crash-Safe Saves (Work is never lost)", style = MaterialTheme.typography.bodyMedium)
                    Text("• Zero Dangerous Runtime Permissions Required", style = MaterialTheme.typography.bodyMedium)
                }
            }

            Spacer(modifier = Modifier.height(16.dp))

            // Licensing
            Text(
                text = "Licensing",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold
            )
            Spacer(modifier = Modifier.height(4.dp))
            Text(
                text = "PdfCraft is free and open-source software, dual-licensed under the MIT License and the Apache License, Version 2.0.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )

            Spacer(modifier = Modifier.height(16.dp))

            // Third-Party Acknowledgments
            Text(
                text = "Third-Party Acknowledgments",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold
            )
            Spacer(modifier = Modifier.height(8.dp))
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Column {
                    Text("Lucide Icons", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
                    Text("ISC License. Copyright (c) Lucide Contributors.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.secondary)
                }
                Column {
                    Text("Open Typography", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
                    Text("Inter, JetBrains Mono, Dancing Script licensed under the SIL Open Font License 1.1.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.secondary)
                }
                Column {
                    Text("Rust Ecosystem", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
                    Text("Built with Mozilla UniFFI, Vello CPU, Hayro, and open-source Rust crates.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.secondary)
                }
            }
        }
    }
}

