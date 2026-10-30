package com.storytold.pdfcraft.kit

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import uniffi.pdfcraft_ffi.FfiRedactionFinding

/**
 * Interface for document redactions with audit verification
 * and an explicit irreversible-action confirmation dialog (Phase 6 / MOB-6.2).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PdfRedactionView(
    document: PdfDocument,
    @Suppress("UNUSED_PARAMETER") pageIndex: Int = 0,
    onDismiss: () -> Unit,
    onApplied: () -> Unit,
    modifier: Modifier = Modifier
) {
    var searchText by remember { mutableStateOf("") }
    var overlayText by remember { mutableStateOf("REDACTED") }
    var showConfirmApply by remember { mutableStateOf(false) }
    var auditFindings by remember { mutableStateOf<List<FfiRedactionFinding>>(emptyList()) }
    var statusMessage by remember { mutableStateOf<String?>(null) }
    var isAuditPassing by remember { mutableStateOf(true) }
    var errorMessage by remember { mutableStateOf<String?>(null) }
    var isProcessing by remember { mutableStateOf(false) }

    fun performAudit() {
        try {
            val findings = document.auditRedactions()
            auditFindings = findings
            if (findings.isEmpty()) {
                statusMessage = "Audit passed: No unapplied marks or insecure opaque covers found."
                isAuditPassing = true
            } else {
                statusMessage = "${findings.size} finding(s) detected. Pending redactions must be applied."
                isAuditPassing = false
            }
            errorMessage = null
        } catch (e: Throwable) {
            errorMessage = e.message
        }
    }

    LaunchedEffect(Unit) {
        performAudit()
    }

    Surface(
        shape = RoundedCornerShape(16.dp),
        tonalElevation = 6.dp,
        modifier = modifier.fillMaxWidth()
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(20.dp)
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Text(
                    text = "Redact Content",
                    style = MaterialTheme.typography.titleLarge,
                    fontWeight = FontWeight.Bold
                )
                TextButton(onClick = onDismiss) {
                    Text("Close")
                }
            }

            Spacer(modifier = Modifier.height(12.dp))

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
                        modifier = Modifier.padding(8.dp)
                    )
                }
                Spacer(modifier = Modifier.height(8.dp))
            }

            statusMessage?.let { status ->
                Surface(
                    shape = RoundedCornerShape(8.dp),
                    color = if (isAuditPassing) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.errorContainer,
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Text(
                        text = status,
                        color = if (isAuditPassing) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onErrorContainer,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(8.dp)
                    )
                }
                Spacer(modifier = Modifier.height(8.dp))
            }

            OutlinedTextField(
                value = searchText,
                onValueChange = { searchText = it },
                label = { Text("Text to redact") },
                placeholder = { Text("Search word or pattern...") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )

            Spacer(modifier = Modifier.height(8.dp))

            OutlinedTextField(
                value = overlayText,
                onValueChange = { overlayText = it },
                label = { Text("Overlay label (optional)") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )

            Spacer(modifier = Modifier.height(12.dp))

            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = {
                        if (searchText.isNotBlank()) {
                            isProcessing = true
                            try {
                                val count = document.markRedactionText(searchText.trim(), overlayText.ifBlank { null })
                                statusMessage = "Marked $count occurrences for redaction."
                                performAudit()
                            } catch (e: Throwable) {
                                errorMessage = e.message
                            } finally {
                                isProcessing = false
                            }
                        }
                    },
                    enabled = searchText.isNotBlank() && !isProcessing
                ) {
                    Text("Mark Text")
                }

                OutlinedButton(onClick = { performAudit() }) {
                    Text("Run Audit")
                }

                OutlinedButton(onClick = {
                    try {
                        document.clearRedactions()
                        performAudit()
                    } catch (e: Throwable) {
                        errorMessage = e.message
                    }
                }) {
                    Text("Clear")
                }
            }

            Spacer(modifier = Modifier.height(16.dp))

            Text(
                text = "Audit Findings & Pending Marks (${auditFindings.size})",
                style = MaterialTheme.typography.titleSmall,
                fontWeight = FontWeight.SemiBold
            )

            Spacer(modifier = Modifier.height(8.dp))

            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 200.dp)
            ) {
                if (auditFindings.isEmpty()) {
                    item {
                        Text(
                            text = "No pending marks or faux redactions found.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.secondary
                        )
                    }
                } else {
                    items(auditFindings) { finding ->
                        Column(modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
                            Text(
                                text = "Page ${finding.page}: ${finding.kind}",
                                style = MaterialTheme.typography.bodyMedium,
                                fontWeight = FontWeight.Medium
                            )
                            if (finding.coveredText.isNotBlank()) {
                                Text(
                                    text = "Covered Text: \"${finding.coveredText}\"",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.secondary
                                )
                            }
                            HorizontalDivider(modifier = Modifier.padding(top = 4.dp))
                        }
                    }
                }
            }

            Spacer(modifier = Modifier.height(16.dp))

            Button(
                onClick = { showConfirmApply = true },
                colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error),
                modifier = Modifier.fillMaxWidth()
            ) {
                Text("Permanently Apply Redactions")
            }
        }
    }

    if (showConfirmApply) {
        AlertDialog(
            onDismissRequest = { showConfirmApply = false },
            title = { Text("Irreversible Redaction") },
            text = {
                Text(
                    "Applying redactions permanently removes the text, vectors, and image pixels beneath the redaction rectangles. " +
                        "This operation cannot be recovered from the output file once saved. Continue?"
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        showConfirmApply = false
                        try {
                            val res = document.applyRedactions()
                            statusMessage = "Redacted ${res.appliedCount} regions. Audit passed: ${res.auditPassed}."
                            performAudit()
                            onApplied()
                        } catch (e: Throwable) {
                            errorMessage = e.message
                        }
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)
                ) {
                    Text("Apply & Remove Content")
                }
            },
            dismissButton = {
                TextButton(onClick = { showConfirmApply = false }) {
                    Text("Cancel")
                }
            }
        )
    }
}
