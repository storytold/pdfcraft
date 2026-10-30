package com.storytold.pdfcraft.kit

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp

/**
 * Dialog allowing fast page jumping using numeric text input or a scrub slider.
 */
@Composable
fun PdfGoToPageDialog(
    currentPage: Int,
    pageCount: Int,
    onPageSelected: (Int) -> Unit,
    onDismiss: () -> Unit
) {
    var pageInput by remember { mutableStateOf((currentPage + 1).toString()) }
    var sliderValue by remember { mutableFloatStateOf((currentPage + 1).toFloat()) }
    var errorMessage by remember { mutableStateOf<String?>(null) }

    Surface(
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surface,
        modifier = Modifier
            .fillMaxWidth()
            .padding(24.dp)
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally
        ) {
            Text(
                text = "Go to Page",
                style = MaterialTheme.typography.titleLarge,
                fontWeight = FontWeight.Bold
            )

            Spacer(modifier = Modifier.height(16.dp))

            Text(
                text = "Page ${sliderValue.toInt()} of $pageCount",
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.primary
            )

            Spacer(modifier = Modifier.height(8.dp))

            if (pageCount > 1) {
                Slider(
                    value = sliderValue,
                    onValueChange = {
                        sliderValue = it
                        pageInput = it.toInt().toString()
                        errorMessage = null
                    },
                    valueRange = 1f..pageCount.toFloat(),
                    steps = if (pageCount > 2) pageCount - 2 else 0,
                    modifier = Modifier.fillMaxWidth()
                )
            }

            Spacer(modifier = Modifier.height(8.dp))

            OutlinedTextField(
                value = pageInput,
                onValueChange = { str ->
                    pageInput = str
                    errorMessage = null
                    val num = str.toIntOrNull()
                    if (num != null && num in 1..pageCount) {
                        sliderValue = num.toFloat()
                    }
                },
                label = { Text("Page Number (1 - $pageCount)") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.fillMaxWidth()
            )

            errorMessage?.let { err ->
                Spacer(modifier = Modifier.height(6.dp))
                Text(
                    text = err,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall
                )
            }

            Spacer(modifier = Modifier.height(20.dp))

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.End,
                verticalAlignment = Alignment.CenterVertically
            ) {
                TextButton(onClick = onDismiss) {
                    Text("Cancel")
                }
                Spacer(modifier = Modifier.width(8.dp))
                Button(
                    onClick = {
                        val num = pageInput.toIntOrNull()
                        if (num == null || num !in 1..pageCount) {
                            errorMessage = "Please enter a valid page number between 1 and $pageCount"
                        } else {
                            onPageSelected(num - 1)
                        }
                    }
                ) {
                    Text("Go")
                }
            }
        }
    }
}

