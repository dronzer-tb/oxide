package dev.oxide.plugin;

import dev.oxide.plugin.provenance.ChunkProvenance;
import dev.oxide.plugin.provenance.LiveProvenance;
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
 * Tracks each online player's current chunk and pushes an action-bar update
 * on chunk change, when that player has the debug overlay toggled on.
 *
 * Threading (deliberate, read this before touching this class):
 * In Folia, entity-bound events such as PlayerMoveEvent are dispatched on
 * the region thread that currently owns that entity — the event handlers
 * below already run on the correct thread for the player that moved, with
 * no scheduling needed to reach that point safely.
 *
 * The one Player-touching call we make anyway — sendActionBar — is still
 * routed through {@code player.getScheduler()} (the Folia per-entity
 * scheduler), not called inline, because:
 *   1. it's the sanctioned way to guarantee an entity-touching task runs on
 *      that entity's *current* owning thread, independent of exactly which
 *      thread today's event dispatch happens to hand us into;
 *   2. its "retired" callback is a clean no-op if the player disconnects or
 *      is removed in the same tick, instead of risking a send against a
 *      dead/reparented entity from a now-stale thread reference.
 * We never use Bukkit.getScheduler() in this plugin: the global/async
 * scheduler has no concept of "the thread that currently owns this
 * player", and using it for entity-touching work is exactly the class of
 * Folia bug that produces intermittent, hard-to-reproduce region-thread
 * crashes instead of a clean, obvious failure.
 */
public final class ChunkTracker implements Listener {

    private final Plugin plugin;
    private final DebugState debugState;
    private final LiveProvenance liveProvenance;

    // Last chunk key (packed x,z) seen per player. ConcurrentHashMap
    // because different players are very likely owned by different region
    // threads at the same instant, all reading/writing this map.
    private final Map<UUID, Long> lastChunk = new ConcurrentHashMap<>();

    public ChunkTracker(Plugin plugin, DebugState debugState, LiveProvenance liveProvenance) {
        this.plugin = plugin;
        this.debugState = debugState;
        this.liveProvenance = liveProvenance;
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
        if (!debugState.isEnabled(player.getUniqueId())) {
            return;
        }
        pushUpdate(player, chunkX, chunkZ);
    }

    private void pushUpdate(Player player, int chunkX, int chunkZ) {
        // Reads the mark this server wrote when it generated the chunk (see LiveProvenance) --
        // an in-memory persistent-data read on the chunk the player is standing in, which is
        // loaded by definition. No file I/O, and unlike the sidecar lookup it reflects what
        // actually generated this chunk on this server.
        ChunkProvenance provenance = liveProvenance.of(player.getChunk());

        player.getScheduler().run(plugin, task -> player.sendActionBar(ActionBarPresenter.render(provenance)),
                null /* retired: player already gone, nothing to do */);
    }
}
