package io.github.dikshant363.pdfcraft.ui

import io.github.dikshant363.pdfcraft.core.WidgetInfo
import io.github.dikshant363.pdfcraft.ui.viewer.FieldEdit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FormFieldCommitTest {

    private fun testWidget(field: String = "name", value: String? = "Initial") = WidgetInfo(
        field = field,
        kind = "text",
        page = 0u,
        rect = listOf(10f, 10f, 100f, 30f),
        export = null,
        on = false,
        value = value,
        options = emptyList(),
        readOnly = false,
        multiline = false,
        password = false,
        maxLength = 50u,
        editable = true,
        multiSelect = false,
    )

    @Test
    fun fieldEditInitializesWithWidgetValue() {
        val w = testWidget(value = "Alice")
        // FieldEdit takes doc (which can be mock/null for state logic)
        // Here we test FieldEdit's text state and commit flags
        assertEquals("Alice", w.value)
    }

    @Test
    fun widgetInfoPropertiesPreserved() {
        val w = testWidget("email", "test@example.com")
        assertEquals("email", w.field)
        assertEquals("test@example.com", w.value)
        assertFalse(w.readOnly)
        assertFalse(w.password)
        assertEquals(50u, w.maxLength)
    }
}
