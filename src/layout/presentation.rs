use super::*;

#[derive(Default)]
pub(crate) struct PresentedLayout {
    pub(crate) panes: Vec<PaneInfo>,
    pub(crate) splits: Vec<SplitBorder>,
}

struct Visibility {
    shown: bool,
    end: usize,
}

impl TileLayout {
    pub(crate) fn resize_pane_presented(
        &mut self,
        pane_id: PaneId,
        nav: NavDirection,
        delta: f32,
        layout: &PresentedLayout,
    ) -> bool {
        let Some(pane) = layout.panes.iter().find(|pane| pane.id == pane_id) else {
            return false;
        };
        let direction = match nav {
            NavDirection::Left | NavDirection::Right => Direction::Horizontal,
            _ => Direction::Vertical,
        };
        let split = nearest_resize_split(&layout.splits, direction, pane.rect, nav).or_else(|| {
            nearest_resize_split(
                &layout.splits,
                direction,
                pane.rect,
                opposite_direction(nav),
            )
        });
        let Some(split) = split else {
            return false;
        };
        let Some(current) = get_ratio_at(&self.root, &split.path) else {
            return false;
        };
        let adjustment = if matches!(nav, NavDirection::Right | NavDirection::Down) {
            delta
        } else {
            -delta
        };
        self.set_ratio_at(&split.path, current + adjustment);
        get_ratio_at(&self.root, &split.path) != Some(current)
    }

    pub(crate) fn presented(
        &self,
        area: Rect,
        focus: Option<PaneId>,
        visible: impl Fn(PaneId) -> bool,
    ) -> PresentedLayout {
        let mut visibility = Vec::with_capacity(self.pane_count().saturating_mul(2));
        mark_visibility(&self.root, &visible, &mut visibility);
        let mut result = PresentedLayout::default();
        collect_presented(
            &self.root,
            0,
            &visibility,
            area,
            focus.unwrap_or(self.focus),
            &mut Vec::new(),
            &mut result,
        );
        if !result.panes.iter().any(|pane| pane.is_focused) {
            if let Some(pane) = result.panes.first_mut() {
                pane.is_focused = true;
            }
        }
        result
    }

    pub(crate) fn first_matching_pane(&self, matches: impl Fn(PaneId) -> bool) -> Option<PaneId> {
        fn find(node: &Node, matches: &impl Fn(PaneId) -> bool) -> Option<PaneId> {
            match node {
                Node::Pane(id) => matches(*id).then_some(*id),
                Node::Split { first, second, .. } => {
                    find(first, matches).or_else(|| find(second, matches))
                }
            }
        }
        find(&self.root, &matches)
    }
}

fn mark_visibility(
    node: &Node,
    visible: &impl Fn(PaneId) -> bool,
    result: &mut Vec<Visibility>,
) -> bool {
    let index = result.len();
    result.push(Visibility {
        shown: false,
        end: 0,
    });
    let shown = match node {
        Node::Pane(id) => visible(*id),
        Node::Split { first, second, .. } => {
            let first = mark_visibility(first, visible, result);
            let second = mark_visibility(second, visible, result);
            first || second
        }
    };
    result[index] = Visibility {
        shown,
        end: result.len(),
    };
    shown
}

fn collect_presented(
    node: &Node,
    index: usize,
    visibility: &[Visibility],
    area: Rect,
    focus: PaneId,
    path: &mut Vec<bool>,
    result: &mut PresentedLayout,
) {
    if !visibility[index].shown {
        return;
    }
    match node {
        Node::Pane(_) => collect_panes(node, area, focus, &mut result.panes),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let first_index = index + 1;
            let second_index = visibility[first_index].end;
            let (first_area, second_area) =
                if visibility[first_index].shown && visibility[second_index].shown {
                    let (first_area, second_area) = split_rect(area, *direction, *ratio);
                    result.splits.push(SplitBorder {
                        pos: match direction {
                            Direction::Horizontal => first_area.right(),
                            Direction::Vertical => first_area.bottom(),
                        },
                        direction: *direction,
                        ratio: *ratio,
                        area,
                        // Dragging must address the shared tree, including collapsed ancestors.
                        path: path.clone(),
                    });
                    (first_area, second_area)
                } else {
                    (area, area)
                };
            path.push(false);
            collect_presented(
                first,
                first_index,
                visibility,
                first_area,
                focus,
                path,
                result,
            );
            path.pop();
            path.push(true);
            collect_presented(
                second,
                second_index,
                visibility,
                second_area,
                focus,
                path,
                result,
            );
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested_layout() -> (TileLayout, [PaneId; 3]) {
        let ids = [PaneId::alloc(), PaneId::alloc(), PaneId::alloc()];
        let layout = TileLayout::from_saved(
            Node::Split {
                direction: Direction::Horizontal,
                ratio: 0.5,
                first: Box::new(Node::Pane(ids[0])),
                second: Box::new(Node::Split {
                    direction: Direction::Vertical,
                    ratio: 0.5,
                    first: Box::new(Node::Pane(ids[1])),
                    second: Box::new(Node::Pane(ids[2])),
                }),
            },
            ids[0],
        );
        (layout, ids)
    }

    #[test]
    fn hidden_sibling_reflows_without_changing_split_paths_or_shared_focus() {
        let (layout, ids) = nested_layout();
        let projected = layout.presented(Rect::new(0, 0, 100, 40), None, |id| id != ids[0]);
        assert_eq!(projected.panes.len(), 2);
        assert_eq!(projected.panes[0].rect, Rect::new(0, 0, 100, 20));
        assert_eq!(projected.panes[1].rect, Rect::new(0, 20, 100, 20));
        assert_eq!(projected.splits.len(), 1);
        assert_eq!(projected.splits[0].path, vec![true]);
        assert_eq!(projected.splits[0].area, Rect::new(0, 0, 100, 40));
        assert!(projected.panes[0].is_focused);
        assert_eq!(layout.focused(), ids[0]);
        assert_eq!(layout.pane_count(), 3);
    }

    #[test]
    fn hiding_every_pane_produces_an_empty_view() {
        let (layout, _) = nested_layout();
        let projected = layout.presented(Rect::new(0, 0, 100, 40), None, |_| false);
        assert!(projected.panes.is_empty());
        assert!(projected.splits.is_empty());
        assert_eq!(layout.pane_count(), 3);
    }

    #[test]
    fn full_visibility_keeps_native_geometry_and_checks_each_pane_once() {
        let (layout, _) = nested_layout();
        let calls = std::cell::Cell::new(0);
        let area = Rect::new(3, 4, 99, 41);
        let projected = layout.presented(area, None, |_| {
            calls.set(calls.get() + 1);
            true
        });
        assert_eq!(calls.get(), 3);
        let native = layout.panes(area);
        assert_eq!(
            projected
                .panes
                .iter()
                .map(|pane| (pane.id, pane.rect, pane.is_focused))
                .collect::<Vec<_>>(),
            native
                .iter()
                .map(|pane| (pane.id, pane.rect, pane.is_focused))
                .collect::<Vec<_>>()
        );
        let native = layout.splits(area);
        assert_eq!(
            projected
                .splits
                .iter()
                .map(|split| (
                    split.path.clone(),
                    split.pos,
                    split.area,
                    split.direction,
                    split.ratio
                ))
                .collect::<Vec<_>>(),
            native
                .iter()
                .map(|split| (
                    split.path.clone(),
                    split.pos,
                    split.area,
                    split.direction,
                    split.ratio
                ))
                .collect::<Vec<_>>()
        );
    }
}
