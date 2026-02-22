package ai.openobscure.test

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/**
 * Minimal Compose UI for manual testing.
 *
 * Enter text → tap Sanitize → see redacted output.
 * Instrumented tests run via `./gradlew connectedAndroidTest`.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    SanitizeScreen()
                }
            }
        }
    }
}

@Composable
fun SanitizeScreen() {
    var input by remember { mutableStateOf("My card is 4111-1111-1111-1111") }
    var output by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }

    Column(modifier = Modifier.padding(16.dp)) {
        Text(text = "OpenObscure Test", style = MaterialTheme.typography.headlineMedium)
        Spacer(modifier = Modifier.height(16.dp))

        OutlinedTextField(
            value = input,
            onValueChange = { input = it },
            label = { Text("Input text") },
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(modifier = Modifier.height(8.dp))

        Button(onClick = {
            try {
                val testKey = "42".repeat(32)
                val manager = PrivacyManager(fpeKeyHex = testKey)
                val result = manager.sanitize(input)
                output = result.sanitizedText
                error = ""
            } catch (e: Exception) {
                error = e.message ?: "Unknown error"
                output = ""
            }
        }) {
            Text("Sanitize")
        }

        Spacer(modifier = Modifier.height(16.dp))
        if (output.isNotEmpty()) {
            Text(text = "Sanitized:", style = MaterialTheme.typography.labelLarge)
            Text(text = output)
        }
        if (error.isNotEmpty()) {
            Text(text = "Error: $error", color = MaterialTheme.colorScheme.error)
        }
    }
}
