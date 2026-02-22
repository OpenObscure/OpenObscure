package ai.openobscure.test

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import androidx.test.ext.junit.runners.AndroidJUnit4

/**
 * Compose UI tests for SanitizeScreen (Espresso + Compose Testing).
 *
 * Run with:
 *   cd test/apps/android && ./gradlew connectedAndroidTest
 *
 * Requires Compose UI testing dependencies in build.gradle.kts.
 */
@RunWith(AndroidJUnit4::class)
class SanitizeScreenTest {

    @get:Rule
    val composeTestRule = createAndroidComposeRule<MainActivity>()

    @Test
    fun screenDisplaysTitle() {
        composeTestRule.onNodeWithText("OpenObscure Test").assertIsDisplayed()
    }

    @Test
    fun inputFieldHasDefaultText() {
        composeTestRule
            .onNodeWithText("My card is 4111-1111-1111-1111")
            .assertIsDisplayed()
    }

    @Test
    fun sanitizeButtonExists() {
        composeTestRule.onNodeWithText("Sanitize").assertIsDisplayed()
    }

    @Test
    fun sanitizeButtonRemovesPii() {
        // Verify the default input contains PII
        composeTestRule
            .onNodeWithText("My card is 4111-1111-1111-1111")
            .assertIsDisplayed()

        // Tap sanitize
        composeTestRule.onNodeWithText("Sanitize").performClick()

        // Wait for result to appear
        composeTestRule.waitForIdle()

        // The "Sanitized:" label should appear
        composeTestRule.onNodeWithText("Sanitized:").assertIsDisplayed()

        // Original card number should NOT be in the output
        composeTestRule
            .onNodeWithText("4111-1111-1111-1111", substring = true)
            .assertDoesNotExist()
    }

    @Test
    fun clearInputAndSanitizeEmpty() {
        // Clear the input field
        composeTestRule
            .onNode(hasSetTextAction())
            .performTextClearance()

        // Tap sanitize
        composeTestRule.onNodeWithText("Sanitize").performClick()
        composeTestRule.waitForIdle()

        // Should show "Sanitized:" with empty output (no error)
        composeTestRule.onNodeWithText("Sanitized:").assertIsDisplayed()
    }

    @Test
    fun typeCustomTextAndSanitize() {
        // Clear default input and type new text
        composeTestRule
            .onNode(hasSetTextAction())
            .performTextClearance()

        composeTestRule
            .onNode(hasSetTextAction())
            .performTextInput("SSN: 123-45-6789")

        // Tap sanitize
        composeTestRule.onNodeWithText("Sanitize").performClick()
        composeTestRule.waitForIdle()

        // Original SSN should not appear in output
        composeTestRule.onNodeWithText("Sanitized:").assertIsDisplayed()
        composeTestRule
            .onNodeWithText("123-45-6789", substring = true)
            .assertDoesNotExist()
    }

    @Test
    fun inputFieldIsEditable() {
        composeTestRule
            .onNode(hasSetTextAction())
            .assertIsEnabled()
    }

    @Test
    fun noErrorOnValidInput() {
        composeTestRule.onNodeWithText("Sanitize").performClick()
        composeTestRule.waitForIdle()

        // "Error:" text should not appear
        composeTestRule
            .onNodeWithText("Error:", substring = true)
            .assertDoesNotExist()
    }
}
