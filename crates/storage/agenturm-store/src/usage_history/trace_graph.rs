//! Per-field representative reports in one retained trace's explicit parent graph.

use super::{CostCounters, Counters, DayCombined, NativeEvidence};
use agenturm_kernel::{ReportedCost, SpanTokenUsage, TraceSpan, TraceStatus};
use std::collections::{BTreeMap, VecDeque};

const FIELDS: usize = 6;
const COST_BIT: u8 = 1 << FIELDS;
const CORE_USAGE: u8 = 0b11;

#[derive(Clone, Copy)]
pub(super) struct Classification {
    pub agent_run: bool,
    pub reported_error: bool,
}

pub(super) struct TraceMember {
    pub span: TraceSpan,
    /// The member observation's session identity. Combined classification
    /// requires both a claimed target source and this exact session.
    pub session: Option<String>,
    pub classification: Option<Classification>,
    pub usage: Option<SpanTokenUsage>,
    pub cost: Option<ReportedCost>,
}

#[derive(Default)]
pub(super) struct DayTraceReports {
    pub traces: usize,
    pub reported_error_traces: usize,
    pub partial_traces: usize,
    pub traces_missing_exact_cost: usize,
    pub tokens: Counters,
    pub costs: CostCounters,
}

struct Node {
    parent: Option<usize>,
    waiting_children: usize,
    descendants: u8,
}

pub(super) fn summarize(
    members: &[TraceMember],
    complete: bool,
    day: &mut DayTraceReports,
    combined: &mut DayCombined,
    native: &NativeEvidence,
) {
    day.traces += 1;
    day.reported_error_traces += usize::from(members.iter().any(|row| {
        row.span.status == TraceStatus::Error
            || row.classification.is_some_and(|value| value.reported_error)
    }));
    let mut partial = !complete || members.iter().any(|row| row.classification.is_none());
    let by_span: BTreeMap<_, _> = members
        .iter()
        .enumerate()
        .map(|(index, row)| (row.span.span_id.as_str(), index))
        .collect();
    let mut nodes: Vec<_> = members
        .iter()
        .map(|row| {
            let parent = row
                .span
                .parent_span_id
                .as_deref()
                .and_then(|id| by_span.get(id).copied());
            partial |= row.span.parent_span_id.is_some() && parent.is_none();
            Node {
                parent,
                waiting_children: 0,
                descendants: 0,
            }
        })
        .collect();
    for index in 0..nodes.len() {
        if let Some(parent) = nodes[index].parent {
            nodes[parent].waiting_children += 1;
        }
    }
    let mut queue: VecDeque<_> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, row)| (row.waiting_children == 0).then_some(index))
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(index) = queue.pop_front() {
        order.push(index);
        if let Some(parent) = nodes[index].parent {
            nodes[parent].waiting_children -= 1;
            if nodes[parent].waiting_children == 0 {
                queue.push_back(parent);
            }
        }
    }
    if order.len() != members.len() || !complete {
        // A cyclic or bounded-out membership has no defensible leaf set.
        day.partial_traces += 1;
        day.traces_missing_exact_cost += 1;
        return;
    }
    day.partial_traces += usize::from(partial);
    let mut saw_core_usage = false;
    let mut matched_cost = !partial;
    for index in order {
        let member = &members[index];
        // A usage-bearing row attributed to a native source claims that its
        // model calls are also covered by that source's native reports.
        if let Some(target) = member
            .usage
            .as_ref()
            .and_then(|usage| usage.target_source_id.as_deref())
            && let Some(session) = member.session.as_deref()
        {
            combined
                .claims
                .insert((target.to_owned(), session.to_owned()));
        }
        let eligible = member.classification.is_some_and(|value| !value.agent_run);
        let mut values = member.usage.as_ref().map_or([None; FIELDS], fields);
        let mut own = 0;
        if eligible {
            for (field, value) in values.iter_mut().enumerate() {
                if value.is_some() {
                    let bit = 1 << field;
                    own |= bit;
                    if nodes[index].descendants & bit != 0 {
                        *value = None;
                    }
                }
            }
            if member.cost.is_some() {
                own |= COST_BIT;
            }
            if values.iter().any(Option::is_some) {
                day.tokens.add(values);
                // Same-day native evidence for the exact claimed identity
                // makes this row's overlap established, so it is excluded and
                // counted rather than summed into the combined total.
                let overlaps = member
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.target_source_id.as_deref())
                    .zip(member.session.as_deref())
                    .is_some_and(|(target, session)| {
                        native
                            .rows
                            .contains_key(&(target.to_owned(), session.to_owned()))
                    });
                if overlaps {
                    combined.excluded += 1;
                } else {
                    combined.tokens.add(values);
                }
            }
            let chosen_cost = member
                .cost
                .as_ref()
                .filter(|_| nodes[index].descendants & COST_BIT == 0);
            if let Some(cost) = chosen_cost {
                day.costs.add(&cost.currency, cost.nanounits);
                // Native reports carry no cost, so retained cost evidence has
                // no native overlap to exclude.
                combined.costs.add(&cost.currency, cost.nanounits);
            }
            if own & CORE_USAGE & !nodes[index].descendants != 0 {
                saw_core_usage = true;
                matched_cost &= chosen_cost.is_some();
            }
        }
        if let Some(parent) = nodes[index].parent {
            let descendants = nodes[index].descendants;
            // Old unclassified rows can't contribute a subtotal but their
            // reported fields prevent a speculative wrapper total.
            let blockers = if member.classification.is_none() {
                fields_mask(&values) | if member.cost.is_some() { COST_BIT } else { 0 }
            } else {
                0
            };
            nodes[parent].descendants |= descendants | own | blockers;
        }
    }
    day.traces_missing_exact_cost += usize::from(!saw_core_usage || !matched_cost);
}

fn fields_mask(values: &[Option<u64>; FIELDS]) -> u8 {
    values.iter().enumerate().fold(0, |mask, (field, value)| {
        if value.is_some() {
            mask | (1 << field)
        } else {
            mask
        }
    })
}

fn fields(usage: &SpanTokenUsage) -> [Option<u64>; FIELDS] {
    [
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_write_input_tokens,
        usage.reasoning_output_tokens,
        usage.total_tokens,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::TraceSpanIdentity;

    fn member(
        id: char,
        parent: Option<char>,
        classification: Option<Classification>,
        counts: [Option<u64>; 2],
        amount: Option<(&str, u64)>,
    ) -> TraceMember {
        let trace_id = "a".repeat(32);
        let native_id = |id: char| id.to_string().repeat(16);
        let span_id = native_id(id);
        let identity = TraceSpanIdentity {
            trace_id: trace_id.clone(),
            span_id: span_id.clone(),
        };
        TraceMember {
            span: TraceSpan {
                trace_id,
                span_id,
                parent_span_id: parent.map(native_id),
                target_source_id: None,
                target_operation_id: None,
                started_at_unix_ns: 1,
                status: TraceStatus::Unset,
            },
            session: None,
            classification,
            usage: counts.iter().any(Option::is_some).then(|| SpanTokenUsage {
                span: identity.clone(),
                target_source_id: None,
                input_tokens: counts[0],
                output_tokens: counts[1],
                cache_read_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            }),
            cost: amount.map(|(currency, nanounits)| ReportedCost {
                currency: currency.into(),
                nanounits,
                span: Some(identity),
                native_origin: None,
            }),
        }
    }

    #[test]
    fn nested_duplicate_reports_are_removed_per_field_and_currency_stays_exact() {
        let model = Some(Classification {
            agent_run: false,
            reported_error: true,
        });
        let run = Some(Classification {
            agent_run: true,
            reported_error: false,
        });
        let spans = [
            member('1', None, run, [Some(400), Some(500)], Some(("USD", 1000))),
            member(
                '2',
                Some('1'),
                model,
                [Some(100), Some(8)],
                Some(("USD", 99)),
            ),
            member('3', Some('2'), model, [None, None], None),
            member('4', Some('3'), model, [Some(3), None], Some(("EUR", 10))),
            member('5', Some('1'), model, [Some(0), None], Some(("USD", 0))),
        ];
        let mut day = DayTraceReports::default();
        let mut combined = DayCombined::default();
        let native = NativeEvidence::default();
        summarize(&spans, true, &mut day, &mut combined, &native);
        let totals = day.totals();
        assert_eq!(totals.traces, 1);
        assert_eq!(totals.reported_error_traces, 1);
        assert_eq!(totals.partial_traces, 0);
        assert_eq!(totals.tokens.reports, 3);
        assert_eq!(totals.tokens.input_tokens.as_deref(), Some("3"));
        assert_eq!(totals.tokens.output_tokens.as_deref(), Some("8"));
        assert_eq!(totals.tokens.total_tokens, None);
        assert_eq!(totals.costs[0].currency, "EUR");
        assert_eq!(totals.costs[0].nanounits, "10");
        assert_eq!(totals.costs[1].currency, "USD");
        assert_eq!(totals.costs[1].nanounits, "0");
        assert_eq!(totals.traces_missing_exact_cost, 1);
    }

    #[test]
    fn cycles_bounded_graphs_and_old_span_classification_stay_marked_incomplete() {
        let model = Some(Classification {
            agent_run: false,
            reported_error: false,
        });
        let root = || member('1', None, model, [Some(100), None], Some(("USD", 10)));
        let mut day = DayTraceReports::default();
        let mut combined = DayCombined::default();
        let native = NativeEvidence::default();
        summarize(
            &[
                member('2', Some('1'), None, [Some(3), None], Some(("USD", 2))),
                root(),
            ],
            true,
            &mut day,
            &mut combined,
            &native,
        );
        summarize(
            &[
                member('2', Some('3'), model, [Some(7), None], None),
                member('3', Some('2'), model, [Some(9), None], None),
            ],
            true,
            &mut day,
            &mut combined,
            &native,
        );
        summarize(&[root()], false, &mut day, &mut combined, &native);
        let totals = day.totals();
        assert_eq!(totals.traces, 3);
        assert_eq!(totals.partial_traces, 3);
        assert_eq!(totals.traces_missing_exact_cost, 3);
        assert_eq!(totals.tokens.input_tokens, None);
        assert!(totals.costs.is_empty());
    }
}
