package com.kintsugi.engine

import android.graphics.BitmapFactory
import android.os.Bundle
import android.widget.Button
import android.widget.ImageView
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import java.io.File

/**
 * 🏺 The whole app: run the engine, show what it says.
 *
 * There is no engine code here. The body, the BlueGale seam, and the glaze are
 * the same Rust crates the desktop CLI uses; this activity is the *shell* —
 * the part that is allowed to look different on every platform.
 *
 * Files live in the app's private storage, because Android 10+ does not hand
 * out raw filesystem paths to other apps' data. Dropping a real game in is a
 * matter of copying it to `filesDir/game/` (see this folder's README).
 */
class MainActivity : AppCompatActivity() {
    private lateinit var transcript: TextView
    private lateinit var preview: ImageView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        transcript = findViewById(R.id.transcript)
        preview = findViewById(R.id.preview)

        val workDir = File(filesDir, "game").apply { mkdirs() }

        transcript.text = buildString {
            appendLine(EngineBridge.version())
            appendLine()
            appendLine("A tiny synthetic BlueGale-style game can be written here and")
            appendLine("played, so the engine can be tried with no game files of your own.")
            appendLine("Working directory: ${workDir.absolutePath}")
        }

        findViewById<Button>(R.id.run_demo).setOnClickListener {
            run("writing and running the demo game") {
                EngineBridge.demo(workDir.absolutePath)
            }
        }

        findViewById<Button>(R.id.detect).setOnClickListener {
            run("detecting") { EngineBridge.detect(workDir.absolutePath) }
        }

        findViewById<Button>(R.id.play).setOnClickListener {
            run("playing story.bdt") {
                EngineBridge.play(workDir.absolutePath, "story.bdt")
            }
        }

        findViewById<Button>(R.id.upscale).setOnClickListener {
            run("glazing title.bbm 4x (Anime4K-style)") {
                val out = File(workDir, "title-x4.png")
                val report = EngineBridge.upscale(
                    workDir.absolutePath,
                    "title.bbm",
                    4,
                    "anime4k",
                    out.absolutePath,
                )
                // Show the repaired pixels, not just a claim about them.
                BitmapFactory.decodeFile(out.absolutePath)?.let { bitmap ->
                    runOnUiThread { preview.setImageBitmap(bitmap) }
                }
                report
            }
        }
    }

    /**
     * Engine work runs off the UI thread: upscaling a full-screen asset on a
     * phone is real work, and a frozen window would look like a hang.
     */
    private fun run(title: String, body: () -> String) {
        transcript.text = "… $title"
        Thread {
            val text = try {
                body()
            } catch (e: Throwable) {
                // Never a blank screen: a failed repair is information too.
                "error: ${e.message ?: e::class.java.simpleName}"
            }
            runOnUiThread { transcript.text = text }
        }.start()
    }
}
