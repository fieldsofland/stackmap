use crate::model::topology::OrderMode;

use super::ORDER_OPTIONS;

pub(super) fn order_option_index(mode: OrderMode) -> usize {
    ORDER_OPTIONS
        .iter()
        .position(|option| *option == mode)
        .unwrap_or(0)
}
