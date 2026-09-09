package dev.oxide.plugin;

import dev.oxide.plugin.pacside.PacsideManager;
import dev.oxide.plugin.provenance.ChunkProvenance;
import dev.oxide.plugin.provenance.LiveProvenance;
import net.kyori.adventure.text.Component;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.event.player.PlayerMoveEvent;
import org.bukkit.event.player.PlayerQuitEvent;
import org.bukkit.plugin.Plugin;

import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Tracks each online player's current chunk and pushes unified action-bar updates
 * on chunk crossings, when that player has the debug/pacside HUD toggled on.
 */
public final class ChunkTracker implements Listener {

    private final Plugin plugin;
    private final DebugState debugState;
    private final LiveProvenance liveProvenance;
    private final PacsideManager pacsideManager;

    private final Map<UUID, Long> lastChunk = new ConcurrentHashMap<>();

    public ChunkTracker(Plugin plugin, DebugState debugState, LiveProvenance liveProvenance, PacsideManager pacsideManager) {
        this.plugin = plugin;
        this.debugState = debugState;
        this.liveProvenance = liveProvenance;
        this.pacsideManager = pacsideManager;
    }

    private static long chunkKey(int chunkX, int chunkZ) {
        return (((long) chunkX) << 32) ^ (chunkZ & 0xFFFFFFFFL);
    }

    @EventHandler
    public void onJoin(PlayerJoinEvent event) {
        Player player = event.getPlayer();
        lastChunk.put(player.getUniqueId(),
                chunkKey(player.getLocation().getBlockX() >> 4, player.getLocation().getBlockZ() >> 4));
    }

    @EventHandler
    public void onQuit(PlayerQuitEvent event) {
        UUID id = event.getPlayer().getUniqueId();
        lastChunk.remove(id);
        debugState.clear(id);
        if (pacsideManager != null && pacsideManager.getPrefetcher() != null) {
            pacsideManager.getPrefetcher().toggleVisualHud(id, false);
        }
    }

    @EventHandler(ignoreCancelled = true)
    public void onMove(PlayerMoveEvent event) {
        if (event.getTo() == null) {
            return;
        }
        Player player = event.getPlayer();
        int chunkX = event.getTo().getBlockX() >> 4;
        int chunkZ = event.getTo().getBlockZ() >> 4;
        long key = chunkKey(chunkX, chunkZ);
        Long previous = lastChunk.put(player.getUniqueId(), key);
        if (previous != null && previous == key) {
            return; // same chunk, nothing to do
        }
        
        boolean enabled = debugState.isEnabled(player.getUniqueId()) ||
                (pacsideManager != null && pacsideManager.getPrefetcher() != null && pacsideManager.getPrefetcher().hasVisualHud(player.getUniqueId()));

        if (!enabled) {
            return;
        }
        pushUpdate(player, chunkX, chunkZ);
    }

    public void pushUpdate(Player player, int chunkX, int chunkZ) {
        ChunkProvenance provenance = liveProvenance.of(player.getChunk());
        boolean isPrefetched = pacsideManager != null && pacsideManager.getPrefetcher() != null &&
                pacsideManager.getPrefetcher().isChunkPrefetched(chunkX, chunkZ);
        int lookahead = player.isGliding() ? 24 : (player.isSprinting() ? 12 : 6);
        int cachedCount = pacsideManager != null && pacsideManager.getPrefetcher() != null ?
                pacsideManager.getPrefetcher().getCachedSetSize() : 0;

        Component bar = ActionBarPresenter.renderUnified(chunkX, chunkZ, provenance, isPrefetched, lookahead, cachedCount);
        player.getScheduler().run(plugin, task -> player.sendActionBar(bar), null);
    }
}
