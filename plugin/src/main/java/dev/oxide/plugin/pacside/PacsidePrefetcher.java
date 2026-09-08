package dev.oxide.plugin.pacside;

import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.Location;
import org.bukkit.Particle;
import org.bukkit.World;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.EventPriority;
import org.bukkit.event.Listener;
import org.bukkit.event.player.PlayerMoveEvent;
import org.bukkit.plugin.java.JavaPlugin;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Predictive Trajectory Chunk Prefetcher & Radius Loader with Visual Feedback.
 *
 * <p>Tracks player movement vectors (dx, dz) and predictively pre-loads the forward
 * chunk cone into memory before the client arrives, eliminating Elytra loading lag.
 */
public final class PacsidePrefetcher implements Listener {

    private final JavaPlugin plugin;
    private final Map<UUID, Long> lastPrefetch = new ConcurrentHashMap<>();
    private final Set<Long> prefetchedChunks = ConcurrentHashMap.newKeySet();
    private final Set<UUID> visualHudPlayers = ConcurrentHashMap.newKeySet();
    private final AtomicLong totalPrefetched = new AtomicLong();
    private boolean enabled = true;

    public PacsidePrefetcher(JavaPlugin plugin) {
        this.plugin = plugin;
    }

    public boolean isEnabled() {
        return enabled;
    }

    public void setEnabled(boolean enabled) {
        this.enabled = enabled;
    }

    public long getPrefetchedCount() {
        return totalPrefetched.get();
    }

    public boolean isChunkPrefetched(int chunkX, int chunkZ) {
        return prefetchedChunks.contains(chunkKey(chunkX, chunkZ));
    }

    public void toggleVisualHud(UUID playerUuid, boolean on) {
        if (on) {
            visualHudPlayers.add(playerUuid);
        } else {
            visualHudPlayers.remove(playerUuid);
        }
    }

    public boolean hasVisualHud(UUID playerUuid) {
        return visualHudPlayers.contains(playerUuid);
    }

    public static long chunkKey(int chunkX, int chunkZ) {
        return (((long) chunkX) << 32) ^ (chunkZ & 0xFFFFFFFFL);
    }

    /**
     * Asynchronously prefetches and warms up all chunks in a radius around the player.
     *
     * @param player The player requesting the prefetch.
     * @param radiusValue Radius in blocks (if > 64) or in chunks (if <= 64).
     */
    public void prefetchRadius(Player player, int radiusValue) {
        int chunkRadius = radiusValue > 64 ? (radiusValue / 16) : radiusValue;
        chunkRadius = Math.max(1, Math.min(chunkRadius, 128)); // Bound between 1 and 128 chunks (up to 2048 blocks)

        Location loc = player.getLocation();
        World world = loc.getWorld();
        int centerChunkX = loc.getBlockX() >> 4;
        int centerChunkZ = loc.getBlockZ() >> 4;

        int totalChunks = (2 * chunkRadius + 1) * (2 * chunkRadius + 1);
        player.sendMessage(Component.text("[Pacside] Prefetching " + totalChunks + " chunks (radius "
                + (chunkRadius * 16) + " blocks) around you...", NamedTextColor.GOLD));

        long startTime = System.currentTimeMillis();
        AtomicInteger loadedCount = new AtomicInteger();
        List<CompletableFuture<?>> futures = new ArrayList<>(totalChunks);

        for (int dz = -chunkRadius; dz <= chunkRadius; dz++) {
            for (int dx = -chunkRadius; dx <= chunkRadius; dx++) {
                int cx = centerChunkX + dx;
                int cz = centerChunkZ + dz;

                CompletableFuture<?> f = world.getChunkAtAsync(cx, cz, false).thenAccept(chunk -> {
                    if (chunk != null) {
                        loadedCount.incrementAndGet();
                        totalPrefetched.incrementAndGet();
                        prefetchedChunks.add(chunkKey(cx, cz));
                    }
                });
                futures.add(f);
            }
        }

        CompletableFuture.allOf(futures.toArray(new CompletableFuture[0])).thenRun(() -> {
            long duration = System.currentTimeMillis() - startTime;
            double cps = loadedCount.get() * 1000.0 / Math.max(1, duration);
            player.sendMessage(Component.text("[Pacside] Successfully pre-warmed " + loadedCount.get()
                    + " chunks in " + duration + " ms (" + String.format("%.1f", cps)
                    + " CPS)!", NamedTextColor.GREEN));

            // Visual feedback: show green particle ring around player's immediate area
            player.getScheduler().run(plugin, task -> {
                spawnVisualAura(player, 8);
                player.sendActionBar(Component.text("[Pacside] All " + loadedCount.get()
                        + " Chunks Warmed in RAM Cache!", NamedTextColor.GREEN));
            }, null);
        });
    }

    @EventHandler(priority = EventPriority.MONITOR, ignoreCancelled = true)
    public void onPlayerMove(PlayerMoveEvent event) {
        if (!enabled) return;

        Location from = event.getFrom();
        Location to = event.getTo();
        if (to == null) return;

        Player player = event.getPlayer();
        int currentChunkX = to.getBlockX() >> 4;
        int currentChunkZ = to.getBlockZ() >> 4;

        // Visual indicator when walking/flying across chunks
        if (visualHudPlayers.contains(player.getUniqueId()) &&
                ((from.getBlockX() >> 4) != currentChunkX || (from.getBlockZ() >> 4) != currentChunkZ)) {
            boolean prefetched = isChunkPrefetched(currentChunkX, currentChunkZ);
            Component bar = Component.text("Chunk [" + currentChunkX + ", " + currentChunkZ + "]: ", NamedTextColor.GRAY)
                    .append(prefetched ? Component.text("PRE-FETCHED (RAM Hit)", NamedTextColor.GREEN)
                            : Component.text("STREAMED", NamedTextColor.AQUA))
                    .append(Component.text(" | RAM Cache: " + prefetchedChunks.size() + " Chunks", NamedTextColor.DARK_GRAY));
            player.sendActionBar(bar);

            if (prefetched) {
                spawnVisualAura(player, 3);
            }
        }

        double dx = to.getX() - from.getX();
        double dz = to.getZ() - from.getZ();
        double speedSq = dx * dx + dz * dz;

        // Trigger prefetching only if player is moving with meaningful velocity (> 0.25 blocks/tick = > 5 m/s)
        if (speedSq < 0.0625) return;

        UUID uuid = player.getUniqueId();
        long now = System.currentTimeMillis();

        Long last = lastPrefetch.get(uuid);
        if (last != null && (now - last) < 350) {
            return;
        }
        lastPrefetch.put(uuid, now);

        double speed = Math.sqrt(speedSq);
        double dirX = dx / speed;
        double dirZ = dz / speed;

        World world = player.getWorld();
        int lookahead = player.isGliding() ? 16 : (player.isSprinting() ? 10 : 6);

        for (int dist = 1; dist <= lookahead; dist++) {
            int targetX = currentChunkX + (int) Math.round(dirX * dist);
            int targetZ = currentChunkZ + (int) Math.round(dirZ * dist);

            world.getChunkAtAsync(targetX, targetZ, false).thenAccept(chunk -> {
                if (chunk != null) {
                    totalPrefetched.incrementAndGet();
                    prefetchedChunks.add(chunkKey(targetX, targetZ));
                }
            });

            // Wider lateral forward cone (3 lateral steps)
            for (int lat = -2; lat <= 2; lat++) {
                if (lat == 0) continue;
                int sideX = currentChunkX + (int) Math.round((dirX * dist) - (dirZ * lat));
                int sideZ = currentChunkZ + (int) Math.round((dirZ * dist) + (dirX * lat));
                world.getChunkAtAsync(sideX, sideZ, false).thenAccept(chunk -> {
                    if (chunk != null) {
                        totalPrefetched.incrementAndGet();
                        prefetchedChunks.add(chunkKey(sideX, sideZ));
                    }
                });
            }
        }
    }

    private void spawnVisualAura(Player player, int radius) {
        Location loc = player.getLocation().add(0, 0.5, 0);
        World world = player.getWorld();
        for (int angle = 0; angle < 360; angle += 45) {
            double rad = Math.toRadians(angle);
            double x = loc.getX() + radius * Math.cos(rad);
            double z = loc.getZ() + radius * Math.sin(rad);
            world.spawnParticle(Particle.HAPPY_VILLAGER, x, loc.getY(), z, 1, 0, 0, 0, 0);
        }
    }
}
