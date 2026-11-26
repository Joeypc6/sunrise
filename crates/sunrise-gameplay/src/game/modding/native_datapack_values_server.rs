//! Server-side datapack vanilla-value table, tags, and obstruction queries.

use crate::game::ServerGame;
use mod_api::{
    BlockKey, BlockPredicateValue, BlockState, BlockStateProviderValue, DatapackValueError,
    HolderSetValue, IntProviderValue, LevelRef, RandomSource as ModRandomSource,
};
use std::collections::{HashMap, HashSet};
use sunrise_core::geometry::{Aabb, Vec3};
use sunrise_gameplay_common::game::Dim;
use sunrise_gameplay_common::game::native_datapack_values::{
    DatapackValuesSide, DatapackVanillaValues, GameDatapackValues,
    datapack_active_world_entities_unobstructed,
};
use sunrise_gameplay_common::game::native_level_refs_common::NativeLevelRefsCommonGameExt1 as _;
use sunrise_gameplay_common::game::native_state_shapes::collision_shape;
use sunrise_gameplay_common::game::tables::GameItems as _;
use sunrise_level::world::World;
use sunrise_world::registry::BlockStateId;
use sunrise_world::worldgen::placement::PlacementQueryError;

impl<'g> DatapackValuesSide<'g> for ServerGame<'g> {
    fn active_datapack_vanilla_values(&self) -> Option<&DatapackVanillaValues> {
        self.common.mods.datapack_vanilla_values.as_deref()
    }

    fn datapack_block_tags(&self) -> &HashMap<String, HashSet<String>> {
        &self.items().block_tags
    }

    /// `EntityGetter.isUnobstructed(null, Shapes.block().move(pos))`: the unit cell at
    /// `position` through [`ServerGame::is_shape_unobstructed`].
    fn is_datapack_block_unobstructed(
        &self,
        level: LevelRef,
        world: &World,
        position: [i32; 3],
    ) -> Result<bool, PlacementQueryError> {
        let cell = sunrise_core::geometry::VoxelShape::block().moved(Vec3::new(
            f64::from(position[0]),
            f64::from(position[1]),
            f64::from(position[2]),
        ));
        self.is_shape_unobstructed(level, world, &cell)
    }
}

impl ServerGame<'_> {
    /// `EntityGetter.isUnobstructed(null, shape)` for a world-space `shape`: checks every live
    /// non-spectator player (including LAN guests and dedicated-server players) and every
    /// `blocksBuilding` entity in `level` (active or parked). Mobs come from the level's entity
    /// lookup by the shape's bounds, as `EntityGetter.getEntities(source, shape.bounds())` does,
    /// not from a walk over every mob. An empty shape is unobstructed.
    pub(in crate::game) fn is_shape_unobstructed(
        &self,
        level: LevelRef,
        world: &World,
        shape: &sunrise_core::geometry::VoxelShape,
    ) -> Result<bool, PlacementQueryError> {
        let Some(cell) = shape.bounds() else {
            return Ok(true);
        };
        let mut obstructs = |blocker: Aabb| {
            sunrise_sim::mobs::spawn_selection::blocks_spawn_shape(shape, cell, blocker)
        };
        let dim_id = level.dimension_id();
        let dim = Dim::from_id(dim_id);
        let is_active = std::ptr::eq(world, &self.common.world);

        if is_active
            && !self.server.lan_acting_fake
            && !self.common.spectator()
            && self
                .common
                .player
                .as_ref()
                .is_some_and(|p| obstructs(p.bounds()))
        {
            return Ok(false);
        }
        if self.server.lan_acting.is_some() {
            if self
                .server
                .lan_building_boxes
                .0
                .iter(dim)
                .any(&mut obstructs)
            {
                return Ok(false);
            }
        } else if let Some(host) = self.server.lan.as_ref()
            && host
                .hub
                .guests
                .simulation_iter()
                .filter(|g| g.dimension == dim_id && g.loaded && !g.dead && !g.spectator())
                .any(|g| obstructs(g.bounds()))
        {
            return Ok(false);
        }

        if is_active {
            return datapack_active_world_entities_unobstructed(
                self,
                level,
                cell,
                obstructs,
                |_| false,
            );
        }

        if let Some(parked) = self.server.levels.0.parked_of(dim_id) {
            return Ok(!parked.any_building_blocker(obstructs));
        }

        Ok(true)
    }
}

impl ServerGame<'_> {
    /// `LevelReader.isUnobstructed(state, pos, CollisionContext.empty())` in a server level:
    /// whether no `blocksBuilding` entity is in the supplied state's collision shape at `pos`.
    pub(in crate::game) fn level_is_unobstructed(
        &self,
        level: LevelRef,
        pos: [i32; 3],
        state: BlockStateId,
    ) -> Result<bool, mod_api::LevelReadError> {
        let world = self.native_level_world_result(level)?;
        let shape = collision_shape(self.common.collision, world, state, pos).moved(Vec3::new(
            f64::from(pos[0]),
            f64::from(pos[1]),
            f64::from(pos[2]),
        ));
        self.is_shape_unobstructed(level, world, &shape)
            .map_err(|_| mod_api::LevelReadError::Unavailable)
    }

    pub(in crate::game) fn datapack_int_provider_sample(
        &self,
        value: IntProviderValue,
        random: &mut dyn ModRandomSource,
    ) -> Result<i32, DatapackValueError> {
        GameDatapackValues::datapack_int_provider_sample(self, value, random)
    }

    pub(in crate::game) fn datapack_block_state_provider_state(
        &self,
        value: BlockStateProviderValue,
        level: LevelRef,
        random: &mut dyn ModRandomSource,
        pos: [i32; 3],
    ) -> Result<BlockState, DatapackValueError> {
        GameDatapackValues::datapack_block_state_provider_state(self, value, level, random, pos)
    }

    pub(in crate::game) fn datapack_block_predicate_test(
        &self,
        value: BlockPredicateValue,
        level: LevelRef,
        pos: [i32; 3],
    ) -> Result<bool, DatapackValueError> {
        GameDatapackValues::datapack_block_predicate_test(self, value, level, pos)
    }

    pub(in crate::game) fn datapack_holder_set_contains(
        &self,
        value: HolderSetValue,
        block: BlockKey,
    ) -> Result<bool, DatapackValueError> {
        GameDatapackValues::datapack_holder_set_contains(self, value, block)
    }
}
