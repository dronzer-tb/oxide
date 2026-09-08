package dev.oxide.plugin.pacside;

import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import net.kyori.adventure.text.format.TextDecoration;
import org.bukkit.Bukkit;
import org.bukkit.Location;
import org.bukkit.Particle;
import org.bukkit.World;
import org.bukkit.entity.Player;
import org.bukkit.plugin.java.JavaPlugin;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Multi-directional high-speed flight load simulator.
 * Spawns N simulated Elytra trajectories radiating in 360 degrees,
 * driving real-time async chunk generation (Oxide) and predictive off-heap packet prefetching (Pacside).
 */
public final class FlightStressSimulator {

    private final JavaPlugin plugin;
    private final PacsideManager pacsideManager;
    private final AtomicBoolean running = new AtomicBoolean(false);
    private final List<VirtualFlyer> flyers = new ArrayList<>();

    private final AtomicInteger generatedChunks = new AtomicInteger(0);
    private final AtomicInteger prefetchedChunks = new AtomicInteger(0);
    private final AtomicLong startTime = new AtomicLong(0);

    public FlightStressSimulator(JavaPlugin plugin, PacsideManager pacsideManager) {
        this.plugin = plugin;
        this.pacsideManager = pacsideManager;
    }

    public boolean isRunning() {
        return running.get();
    }

    public void start(World world, Location origin, int flyerCount, double speedMps, int durationSeconds) {
        if (running.getAndSet(true)) {
            return;
        }

        flyers.clear();
        generatedChunks.set(0);
        prefetchedChunks.set(0);
        startTime.set(System.currentTimeMillis());

        double angleStep = (2.0 * Math.PI) / flyerCount;
        for (int i = 0; i < flyerCount; i++) {
            double angle = i * angleStep;
            double vx = Math.cos(angle) * speedMps;
            double vz = Math.sin(angle) * speedMps;
            flyers.add(new VirtualFlyer(origin.getX(), origin.getY(), origin.getZ(), vx, vz, angle));
        }

        Bukkit.broadcast(Component.text("⚡ [Oxide Stress] Started multi-directional flight simulation: "
                + flyerCount + " flyers @ " + speedMps + " m/s for " + durationSeconds + "s", NamedTextColor.GOLD, TextDecoration.BOLD));

        // Ticking loop running on global region scheduler
        plugin.getServer().getGlobalRegionScheduler().runAtFixedRate(plugin, task -> {
            if (!running.get()) {
                task.cancel();
                return;
            }

            long elapsedSec = (System.currentTimeMillis() - startTime.get()) / 1000;
            if (elapsedSec >= durationSeconds) {
                stop();
                task.cancel();
                return;
            }

            double dt = 0.05; // 1 tick = 50ms
            for (VirtualFlyer flyer : flyers) {
                flyer.step(dt);

                int cx = ((int) Math.floor(flyer.x)) >> 4;
                int cz = ((int) Math.floor(flyer.z)) >> 4;

                if (cx != flyer.lastChunkX || cz != flyer.lastChunkZ) {
                    flyer.lastChunkX = cx;
                    flyer.lastChunkZ = cz;

                    // Trigger async chunk load / generation
                    if (!world.isChunkLoaded(cx, cz)) {
                        world.getChunkAtAsync(cx, cz, true).thenAccept(chunk -> {
                            generatedChunks.incrementAndGet();
                        });
                    }

                    // Trigger Pacside lookahead cone (8 chunks ahead along trajectory)
                    for (int dist = 1; dist <= 8; dist++) {
                        int lookaheadX = cx + (int) Math.round((flyer.vx / speedMps) * dist);
                        int lookaheadZ = cz + (int) Math.round((flyer.vz / speedMps) * dist);

                        if (!world.isChunkLoaded(lookaheadX, lookaheadZ)) {
                            world.getChunkAtAsync(lookaheadX, lookaheadZ, true).thenAccept(chunk -> {
                                prefetchedChunks.incrementAndGet();
                            });
                        }
                    }
                }
            }

            // Status broadcast every 5 seconds
            if (elapsedSec > 0 && elapsedSec % 5 == 0 && System.currentTimeMillis() % 1000 < 60) {
                double cps = (double) (generatedChunks.get() + prefetchedChunks.get()) / Math.max(1, elapsedSec);
                Bukkit.broadcast(Component.text(String.format("✈ [Oxide Stress] Time: %ds | Active: %d flyers | Chunks: %d gen, %d prefetch | Rate: %.1f CPS",
                        elapsedSec, flyerCount, generatedChunks.get(), prefetchedChunks.get(), cps), NamedTextColor.AQUA));
            }
        }, 1L, 1L);
    }

    public void stop() {
        if (!running.getAndSet(false)) {
            return;
        }

        long elapsedSec = Math.max(1, (System.currentTimeMillis() - startTime.get()) / 1000);
        int total = generatedChunks.get() + prefetchedChunks.get();
        double avgCps = (double) total / elapsedSec;

        Bukkit.broadcast(Component.text(String.format("🏁 [Oxide Stress] Simulation Complete! Duration: %ds | Total Chunks: %d | Avg Throughput: %.1f CPS",
                elapsedSec, total, avgCps), NamedTextColor.GREEN, TextDecoration.BOLD));
    }

    private static class VirtualFlyer {
        double x, y, z;
        double vx, vz;
        double angle;
        int lastChunkX = Integer.MIN_VALUE;
        int lastChunkZ = Integer.MIN_VALUE;

        VirtualFlyer(double x, double y, double z, double vx, double vz, double angle) {
            this.x = x;
            this.y = y;
            this.z = z;
            this.vx = vx;
            this.vz = vz;
            this.angle = angle;
        }

        void step(double dt) {
            x += vx * dt;
            z += vz * dt;
        }
    }
}
