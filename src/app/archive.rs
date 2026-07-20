use crate::model::BranchId;

pub(super) fn archive_range_slice<'a>(
    eligible: &'a [BranchId],
    anchor: &BranchId,
    endpoint: &BranchId,
) -> Option<&'a [BranchId]> {
    let anchor = eligible.iter().position(|branch| branch == anchor)?;
    let endpoint = eligible.iter().position(|branch| branch == endpoint)?;
    let (start, end) = if anchor <= endpoint {
        (anchor, endpoint)
    } else {
        (endpoint, anchor)
    };
    Some(&eligible[start..=end])
}
