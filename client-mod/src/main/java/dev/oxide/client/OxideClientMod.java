package dev.oxide.client;

import dev.oxide.client.render.OxideNativeMesher;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.File;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;

/**
 * Client mod entrypoint for Oxide Native Rust Renderer.
 * Initialized during Fabric/NeoForge client bootstrap.
 */
public final class OxideClientMod {

    public static final String MOD_ID = "oxide-client";
    public static final Logger LOGGER = LoggerFactory.getLogger(MOD_ID);
    private static boolean initialized = false;

    public static void onInitializeClient() {
        if (initialized) return;

        LOGGER.info("[Oxide Client] Bootstrapping Native Rust GPU Meshing Pipeline...");

        try {
            loadNativeLibrary();
            OxideNativeMesher.init();
            LOGGER.info("[Oxide Client] Native Rust SIMD Greedy Mesher & Panama FFI successfully initialized!");
            initialized = true;
        } catch (Throwable t) {
            LOGGER.error("[Oxide Client] Failed to initialize native Rust rendering pipeline, falling back to Java renderer", t);
        }
    }

    private static void loadNativeLibrary() throws Exception {
        String os = System.getProperty("os.name").toLowerCase();
        String arch = System.getProperty("os.arch").toLowerCase();

        String libName;
        String resourcePath;

        if (os.contains("win")) {
            libName = "oxide_render.dll";
            resourcePath = "/natives/windows-x86_64/" + libName;
        } else if (os.contains("mac") || os.contains("darwin")) {
            libName = "liboxide_render.dylib";
            resourcePath = arch.contains("aarch64") || arch.contains("arm") ?
                    "/natives/macos-aarch64/" + libName : "/natives/macos-x86_64/" + libName;
        } else {
            libName = "liboxide_render.so";
            resourcePath = arch.contains("aarch64") || arch.contains("arm") ?
                    "/natives/linux-aarch64/" + libName : "/natives/linux-x86_64/" + libName;
        }

        File tempLib = File.createTempFile("oxide_render_", "_" + libName);
        tempLib.deleteOnExit();

        try (InputStream in = OxideClientMod.class.getResourceAsStream(resourcePath)) {
            if (in != null) {
                Files.copy(in, tempLib.toPath(), StandardCopyOption.REPLACE_EXISTING);
                System.load(tempLib.getAbsolutePath());
                LOGGER.info("[Oxide Client] Loaded bundled native library from: {}", resourcePath);
                return;
            }
        }

        // Fallback to system library path
        try {
            System.loadLibrary("oxide_render");
            LOGGER.info("[Oxide Client] Loaded native library from system path.");
        } catch (UnsatisfiedLinkError e) {
            LOGGER.warn("[Oxide Client] Could not find bundled binary for {} ({}). Ensure liboxide_render is compiled.", os, arch);
        }
    }
}
