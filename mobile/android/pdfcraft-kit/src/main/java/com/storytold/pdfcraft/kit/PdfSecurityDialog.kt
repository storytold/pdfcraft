package com.storytold.pdfcraft.kit

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp

/**
 * Mobile Document Security & Password Protection dialog.
 */
@Composable
fun PdfSecurityDialog(
    onDismiss: () -> Unit,
    onProtect: (openPassword: String?, permissionsPassword: String?, printing: String, changes: String, copy: Boolean, accessibility: Boolean) -> Unit,
    onUnprotect: () -> Unit,
    modifier: Modifier = Modifier
) {
    var openPassword by remember { mutableStateOf("") }
    var permissionsPassword by remember { mutableStateOf("") }
    var printing by remember { mutableStateOf("full") }
    var changes by remember { mutableStateOf("any-except-extract") }
    var allowCopy by remember { mutableStateOf(true) }
    var allowAccessibility by remember { mutableStateOf(true) }

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
                    text = "Password & Security",
                    style = MaterialTheme.typography.headlineSmall,
                    fontWeight = FontWeight.Bold
                )
                TextButton(onClick = onDismiss) {
                    Text("Cancel")
                }
            }

            Spacer(modifier = Modifier.height(16.dp))

            OutlinedTextField(
                value = openPassword,
                onValueChange = { openPassword = it },
                label = { Text("Document Open Password") },
                placeholder = { Text("Require password to open") },
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )

            Spacer(modifier = Modifier.height(12.dp))

            OutlinedTextField(
                value = permissionsPassword,
                onValueChange = { permissionsPassword = it },
                label = { Text("Permissions Password") },
                placeholder = { Text("Restrict printing and editing") },
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                singleLine = true,
                modifier = Modifier.fillMaxWidth()
            )

            if (permissionsPassword.isNotBlank()) {
                Spacer(modifier = Modifier.height(16.dp))
                Text("Printing Allowed", style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf("none" to "None", "low-resolution" to "Low Res", "full" to "Full").forEach { (valKey, label) ->
                        FilterChip(
                            selected = printing == valKey,
                            onClick = { printing = valKey },
                            label = { Text(label) }
                        )
                    }
                }

                Spacer(modifier = Modifier.height(12.dp))
                Text("Changes Allowed", style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf("none" to "None", "fill-sign" to "Fill & Sign", "any-except-extract" to "Any").forEach { (valKey, label) ->
                        FilterChip(
                            selected = changes == valKey,
                            onClick = { changes = valKey },
                            label = { Text(label) }
                        )
                    }
                }

                Spacer(modifier = Modifier.height(12.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(
                        checked = allowCopy,
                        onCheckedChange = { allowCopy = it }
                    )
                    Text("Allow text & graphic copying", style = MaterialTheme.typography.bodyMedium)
                }

                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(
                        checked = allowAccessibility,
                        onCheckedChange = { allowAccessibility = it }
                    )
                    Text("Allow screen reader accessibility", style = MaterialTheme.typography.bodyMedium)
                }
            }

            Spacer(modifier = Modifier.height(20.dp))

            Button(
                onClick = {
                    val open = openPassword.ifBlank { null }
                    val perm = permissionsPassword.ifBlank { null }
                    onProtect(open, perm, printing, changes, allowCopy, allowAccessibility)
                    onDismiss()
                },
                enabled = openPassword.isNotBlank() || permissionsPassword.isNotBlank(),
                modifier = Modifier.fillMaxWidth()
            ) {
                Text("Encrypt & Protect")
            }

            Spacer(modifier = Modifier.height(8.dp))

            OutlinedButton(
                onClick = {
                    onUnprotect()
                    onDismiss()
                },
                modifier = Modifier.fillMaxWidth(),
                colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error)
            ) {
                Text("Remove Password Security")
            }
        }
    }
}
