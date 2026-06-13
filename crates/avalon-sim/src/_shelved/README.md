# Shelved persistent-world modules

These power the *persistent* Avalon (cross-session NPC memory, gossip, factions,
and The Inquiry tribunal). The game is currently an ephemeral single-mission
roguelike (see lib.rs), so they are out of the build. Kept verbatim to revive
later: re-add `mod memory; mod inquiry;` to lib.rs and rewire to Mission.
