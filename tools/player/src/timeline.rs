//! The build animation as a function of time: parts drop into place, step by step.
//!
//! Every part gets its start time up front, so any moment of the build can be
//! shown directly: playing, seeking, jumping between steps or scrubbing
//! backwards all go through [`Playhead::apply`]. (The demo viewer's animation
//! only ever adds parts, so it can't pause or go back.)

use std::ops::Range;

use cgmath::SquareMatrix;
use ldraw::{
    Matrix4, PartAlias,
    color::{Color, ColorCatalog, ColorReference},
};
use ldraw_ir::model::{Model, Object, ObjectId, ObjectInstance};
use ldraw_renderer::{
    Entity,
    display_list::{DisplayList, DisplayListOps},
};
use uuid::Uuid;

/// How the build is paced, in seconds at 1x speed.
#[derive(Clone, Copy, Debug)]
pub struct Pacing {
    /// Delay between consecutive parts of a step...
    pub stagger: f32,
    /// ...but all of a step's parts start within this long.
    pub max_step_spread: f32,
    /// How long a part takes to drop into place.
    pub fall: f32,
    /// Pause after each step.
    pub hold: f32,
    /// Longer builds are sped up to take about this long...
    pub target_duration: f32,
    /// ...but parts never drop faster than this, so they still visibly fall.
    pub min_fall: f32,
    /// How far above its place a part starts falling, in LDraw units.
    pub drop_height: f32,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            stagger: 0.2,
            max_step_spread: 10.0,
            fall: 0.5,
            hold: 0.3,
            target_duration: 60.0,
            min_fall: 0.12,
            drop_height: 300.0,
        }
    }
}

/// One placed part of the model.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: ObjectId,
    pub alias: PartAlias,
    pub matrix: Matrix4,
    pub color: Color,
    /// When the part starts to fall, in seconds.
    pub start: f32,
}

#[derive(Debug, Default)]
pub struct Timeline {
    items: Vec<Item>, // sorted by start
    steps: Vec<f32>,  // when each step starts
    duration: f32,
    fall: f32,
    drop_height: f32,
}

impl Timeline {
    /// The model's parts in building order. Submodels are built in place, with
    /// their own steps, where the parent model uses them (as the demo does).
    /// Parts for which `keep` is false (e.g. files that couldn't be loaded)
    /// are left out.
    pub fn from_model(
        model: &Model<PartAlias>,
        colors: &ColorCatalog,
        pacing: &Pacing,
        keep: impl Fn(&PartAlias) -> bool,
    ) -> Self {
        let mut flattener = Flattener {
            model,
            colors,
            keep: &keep,
            steps: Vec::new(),
            current: Vec::new(),
        };
        let top_color = colors.get(&0).cloned().unwrap_or_default();
        flattener.flatten(
            &model.objects,
            Uuid::nil().into(),
            Matrix4::identity(),
            &top_color,
            0,
        );
        flattener.end_step();
        Self::schedule(flattener.steps, pacing)
    }

    /// Gives every part its start time. Steps are laid out one after another;
    /// a build longer than `pacing.target_duration` is compressed to fit.
    pub fn schedule(steps: Vec<Vec<Item>>, pacing: &Pacing) -> Self {
        let steps: Vec<Vec<Item>> = steps.into_iter().filter(|s| !s.is_empty()).collect();
        let staggers: Vec<f32> = steps
            .iter()
            .map(|s| {
                let n = s.len() as f32;
                if n * pacing.stagger >= pacing.max_step_spread {
                    pacing.max_step_spread / n
                } else {
                    pacing.stagger
                }
            })
            .collect();

        // Spreads and pauses scale down together; drops only down to a
        // minimum, which (for builds with hundreds of steps) may itself shrink
        // so that drops never take more than half of the target duration.
        let count = steps.len() as f32;
        let spread: f32 = steps
            .iter()
            .zip(&staggers)
            .map(|(s, stagger)| (s.len() - 1) as f32 * stagger)
            .sum();
        let natural = spread + count * (pacing.fall + pacing.hold);
        let (scale, fall) = if natural <= pacing.target_duration {
            (1.0, pacing.fall)
        } else {
            let scale = pacing.target_duration / natural;
            let min_fall = pacing
                .min_fall
                .min(pacing.fall)
                .min(0.5 * pacing.target_duration / count);
            if pacing.fall * scale >= min_fall {
                (scale, pacing.fall * scale)
            } else {
                let rest = (pacing.target_duration - count * min_fall).max(0.0);
                (
                    rest / (spread + count * pacing.hold).max(f32::EPSILON),
                    min_fall,
                )
            }
        };
        let hold = pacing.hold * scale;

        let mut timeline = Self {
            items: Vec::with_capacity(steps.iter().map(Vec::len).sum()),
            steps: Vec::with_capacity(steps.len()),
            duration: 0.0,
            fall,
            drop_height: pacing.drop_height,
        };
        let mut time = 0.0;
        for (step, stagger) in steps.into_iter().zip(staggers) {
            let stagger = stagger * scale;
            let count = step.len();
            timeline.steps.push(time);
            for (index, mut item) in step.into_iter().enumerate() {
                item.start = time + index as f32 * stagger;
                timeline.items.push(item);
            }
            time += (count - 1) as f32 * stagger + fall + hold;
        }
        timeline.duration = time;
        timeline
    }

    pub fn duration(&self) -> f32 {
        self.duration
    }

    /// When each step starts (the "chapter" marks), in seconds.
    pub fn steps(&self) -> &[f32] {
        &self.steps
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The step being built at `time` (0-based; 0 when there are no steps).
    pub fn step_at(&self, time: f32) -> usize {
        self.steps.partition_point(|&s| s <= time).saturating_sub(1)
    }

    /// How a falling part looks at `progress` (0..1): its matrix and alpha.
    fn falling(&self, item: &Item, progress: f32) -> (Matrix4, f32) {
        let ease = (progress.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin();
        let mut matrix = item.matrix;
        matrix[3][1] -= (1.0 - ease) * self.drop_height; // -Y is up in LDraw
        let alpha = ease * item.color.color.alpha() as f32 / 255.0;
        (matrix, alpha)
    }
}

struct Flattener<'a> {
    model: &'a Model<PartAlias>,
    colors: &'a ColorCatalog,
    keep: &'a dyn Fn(&PartAlias) -> bool,
    steps: Vec<Vec<Item>>,
    current: Vec<Item>,
}

impl Flattener<'_> {
    const MAX_DEPTH: u32 = 64; // guards against submodels that (indirectly) include themselves

    fn end_step(&mut self) {
        if !self.current.is_empty() {
            self.steps.push(std::mem::take(&mut self.current));
        }
    }

    /// The colour a part or submodel is drawn in: its own, or the one it's
    /// placed with for colour 16. Same rules as `DisplayList::from_model`.
    fn resolve(&self, reference: &ColorReference, current: &Color) -> Color {
        match reference {
            ColorReference::Current => current.clone(),
            ColorReference::Color(c) => c.clone(),
            _ => self.colors.get(&0).cloned().unwrap_or_default(),
        }
    }

    fn flatten(
        &mut self,
        objects: &[Object<PartAlias>],
        parent: ObjectId,
        matrix: Matrix4,
        color: &Color,
        depth: u32,
    ) {
        for object in objects {
            match &object.data {
                ObjectInstance::Step => self.end_step(),
                ObjectInstance::Part(part) if (self.keep)(&part.part) => {
                    let item = Item {
                        id: uuid_xor(parent, object.id),
                        alias: part.part.clone(),
                        matrix: matrix * part.matrix,
                        color: self.resolve(&part.color, color),
                        start: 0.0,
                    };
                    self.current.push(item);
                }
                ObjectInstance::PartGroup(group) if depth < Self::MAX_DEPTH => {
                    if let Some(submodel) = self.model.object_groups.get(&group.group_id) {
                        let group_color = self.resolve(&group.color, color);
                        self.flatten(
                            &submodel.objects,
                            uuid_xor(parent, object.id),
                            matrix * group.matrix,
                            &group_color,
                            depth + 1,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// Unique id per placed part, also for the same submodel used several times
/// (as `DisplayList::from_model` does).
fn uuid_xor(a: ObjectId, b: ObjectId) -> ObjectId {
    let a = Uuid::from(a).to_bytes_le();
    let b = Uuid::from(b).to_bytes_le();
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = a[i] ^ b[i];
    }
    Uuid::from_bytes_le(bytes).into()
}

/// What the display list currently shows of a timeline.
///
/// Items `[0, shown)` are in the display list; of those, `falling` are still
/// on their way down and the rest are in place.
#[derive(Debug, Default)]
pub struct Playhead {
    shown: usize,
    falling: Range<usize>,
    time: Option<f32>,
}

impl Playhead {
    /// Forget what's shown (after the display list was replaced).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Brings `display_list` to how the build looks at `time`. Returns whether
    /// anything changed.
    ///
    /// Going forward only adds parts and moves the falling ones; going back
    /// starts from an empty list. That also keeps clear of two display-list
    /// quirks: a part inserted and re-coloured in the same frame loses the
    /// colour change, and removing a group's last committed part in a frame
    /// drops that group's other pending inserts.
    pub fn apply(
        &mut self,
        timeline: &Timeline,
        time: f32,
        display_list: &mut Entity<DisplayList<ObjectId, PartAlias>>,
    ) -> bool {
        if self.time == Some(time) {
            return false;
        }
        if self.time.is_some_and(|previous| time < previous) {
            *display_list = DisplayList::new().into();
            self.reset();
        }

        let items = timeline.items();
        let shown = items.partition_point(|item| item.start <= time);
        let landed = items.partition_point(|item| item.start + timeline.fall <= time);
        let mut ops = Vec::new();

        // Were falling, now in place: final position and full colour.
        let was_falling = self.falling.start..self.falling.end.min(landed).max(self.falling.start);
        for item in &items[was_falling] {
            ops.push(DisplayListOps::Update {
                key: item.id,
                matrix: item.matrix,
                color: item.color.clone(),
            });
        }
        // Still falling.
        let still_falling = landed.max(self.falling.start);
        for item in &items[still_falling..self.shown.min(shown).max(still_falling)] {
            let (matrix, alpha) = timeline.falling(item, (time - item.start) / timeline.fall);
            ops.push(DisplayListOps::UpdateMatrix {
                key: item.id,
                matrix,
            });
            ops.push(DisplayListOps::UpdateAlpha {
                key: item.id,
                alpha,
            });
        }
        // New since last time: straight into their current state.
        for (index, item) in items.iter().enumerate().take(shown).skip(self.shown) {
            let (matrix, alpha) = if index < landed {
                (item.matrix, None)
            } else {
                let (matrix, alpha) = timeline.falling(item, (time - item.start) / timeline.fall);
                (matrix, Some(alpha))
            };
            ops.push(DisplayListOps::Insert {
                group: item.alias.clone(),
                key: item.id,
                matrix,
                color: item.color.clone(),
                alpha,
            });
        }

        let changed = !ops.is_empty() || self.time.is_none();
        display_list.mutate_all(ops.into_iter());
        self.shown = shown;
        self.falling = landed.min(shown)..shown;
        self.time = Some(time);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldraw_ir::model::{GroupId, ObjectGroup, PartGroupInstance, PartInstance};

    fn part(name: &str, color: ColorReference) -> Object<PartAlias> {
        Object {
            id: Uuid::new_v4().into(),
            data: ObjectInstance::Part(PartInstance {
                matrix: Matrix4::identity(),
                color,
                part: PartAlias::from(name),
            }),
        }
    }

    fn step() -> Object<PartAlias> {
        Object {
            id: Uuid::new_v4().into(),
            data: ObjectInstance::Step,
        }
    }

    fn red() -> Color {
        Color {
            code: 4,
            name: "Red".into(),
            ..Default::default()
        }
    }

    fn items(n: usize) -> Vec<Item> {
        (0..n)
            .map(|i| Item {
                id: Uuid::new_v4().into(),
                alias: PartAlias::from(format!("{i}.dat")),
                matrix: Matrix4::identity(),
                color: Color::default(),
                start: 0.0,
            })
            .collect()
    }

    #[test]
    fn small_builds_keep_their_natural_pace() {
        let timeline = Timeline::schedule(vec![items(3), vec![], items(3)], &Pacing::default());
        // empty steps are dropped; each step: 2 * 0.2 stagger + 0.5 fall + 0.3 hold
        assert_eq!(timeline.steps().len(), 2);
        assert!((timeline.steps()[1] - 1.2).abs() < 1e-5);
        let starts: Vec<f32> = timeline.items().iter().map(|i| i.start).collect();
        for (got, want) in starts.iter().zip([0.0, 0.2, 0.4, 1.2, 1.4, 1.6]) {
            assert!((got - want).abs() < 1e-5, "{starts:?}");
        }
        assert!((timeline.duration() - 2.4).abs() < 1e-5);
    }

    #[test]
    fn long_builds_are_compressed_to_about_a_minute() {
        let steps = (0..300).map(|_| items(20)).collect();
        let timeline = Timeline::schedule(steps, &Pacing::default());
        assert_eq!(timeline.items().len(), 6000);
        assert!(
            (timeline.duration() - 60.0).abs() < 0.01,
            "{}",
            timeline.duration()
        );
        assert!((timeline.fall - 0.1).abs() < 1e-4, "{}", timeline.fall); // 300 drops: half a minute
        let starts: Vec<f32> = timeline.items().iter().map(|i| i.start).collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn builds_with_hundreds_of_steps_still_take_about_a_minute() {
        let few = Timeline::schedule((0..100).map(|_| items(20)).collect(), &Pacing::default());
        assert!((few.fall - 0.12).abs() < 1e-4, "{}", few.fall); // the minimum drop
        let many = Timeline::schedule((0..600).map(|_| items(9)).collect(), &Pacing::default());
        assert!((many.duration() - 60.0).abs() < 0.01, "{}", many.duration());
        assert!((many.fall - 0.05).abs() < 1e-4, "{}", many.fall); // 600 drops: half a minute
    }

    #[test]
    fn step_at_finds_the_chapter() {
        let timeline = Timeline::schedule(vec![items(3), items(3)], &Pacing::default());
        assert_eq!(timeline.step_at(0.0), 0);
        assert_eq!(timeline.step_at(1.19), 0);
        assert_eq!(timeline.step_at(1.2), 1);
        assert_eq!(timeline.step_at(100.0), 1);
        assert_eq!(Timeline::default().step_at(3.0), 0);
    }

    #[test]
    fn submodels_are_built_in_place_with_their_steps_and_colour() {
        let sub_id: GroupId = Uuid::new_v4().into();
        let mut sub = ObjectGroup::new(sub_id, "wheel.ldr".into(), None);
        sub.objects = vec![
            part("3641.dat", ColorReference::Current),
            step(),
            part("4624.dat", ColorReference::Current),
        ];
        let placement = |_| Object {
            id: Uuid::new_v4().into(),
            data: ObjectInstance::PartGroup(PartGroupInstance {
                matrix: Matrix4::identity(),
                color: ColorReference::Color(red()),
                group_id: sub_id,
            }),
        };
        let mut model = Model::<PartAlias>::default();
        model.object_groups.insert(sub_id, sub);
        model.objects = vec![
            part("3001.dat", ColorReference::Color(red())),
            step(),
            placement(0),
            placement(1),
        ];

        let timeline =
            Timeline::from_model(&model, &ColorCatalog::new(), &Pacing::default(), |_| true);
        let names: Vec<&str> = timeline
            .items()
            .iter()
            .map(|i| i.alias.normalized.as_str())
            .collect();
        assert_eq!(
            names,
            ["3001.dat", "3641.dat", "4624.dat", "3641.dat", "4624.dat"]
        );
        // steps: [3001] [3641] [4624, 3641] [4624]
        assert_eq!(timeline.steps().len(), 4);
        // colour 16 inside the submodel takes the colour it was placed with
        assert!(timeline.items()[1..].iter().all(|i| i.color.code == 4));
        // the same submodel used twice still gives unique ids
        let ids: std::collections::HashSet<_> = timeline.items().iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn parts_that_could_not_be_loaded_are_left_out() {
        let model = Model::<PartAlias> {
            objects: vec![
                part("3001.dat", ColorReference::Current),
                step(),
                part("missing.dat", ColorReference::Current),
                step(),
                part("3003.dat", ColorReference::Current),
            ],
            ..Default::default()
        };
        let keep = |alias: &PartAlias| alias.normalized != "missing.dat";
        let timeline = Timeline::from_model(&model, &ColorCatalog::new(), &Pacing::default(), keep);
        assert_eq!(timeline.items().len(), 2);
        assert_eq!(timeline.steps().len(), 2); // its step would be empty: dropped
    }

    #[test]
    fn playhead_shows_parts_as_time_passes_and_rewinds() {
        let timeline = Timeline::schedule(vec![items(3), items(3)], &Pacing::default());
        let mut list: Entity<DisplayList<ObjectId, PartAlias>> = DisplayList::new().into();
        let mut playhead = Playhead::default();
        let shown = |list: &Entity<DisplayList<ObjectId, PartAlias>>| {
            timeline
                .items()
                .iter()
                .filter(|i| list.get_by_key(&i.id).is_some())
                .count()
        };

        assert!(playhead.apply(&timeline, 0.3, &mut list));
        assert_eq!(shown(&list), 2);
        assert!(!playhead.apply(&timeline, 0.3, &mut list));
        playhead.apply(&timeline, 2.4, &mut list);
        assert_eq!(shown(&list), 6);
        assert_eq!(playhead.falling, 6..6);
        playhead.apply(&timeline, 1.3, &mut list); // back into step 2
        assert_eq!(shown(&list), 4);
        assert_eq!(playhead.falling, 3..4);
        playhead.apply(&timeline, 0.0, &mut list);
        assert_eq!(shown(&list), 1); // the first part, just starting to fall
    }
}
