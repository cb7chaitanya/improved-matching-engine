# Invariants

Every invariant has an ID, the decision it comes from, and how it is checked.
- **State**: checked on the book after every command (full checker, tests + shadow replica).
- **Stream**: checked over the event stream (property tests).
- **Static**: enforced by types or compile-time asserts.

## Numbers
| ID | Invariant | From | Check |
|---|---|---|---|
| N1 | Every live order has `1 <= remaining` and `filled + remaining <= max_order_qty` | 001 | State |
| N2 | Every live order's price is in `[min_tick, max_tick]` | 001 | State |
| N3 | `Price` and `Qty` cannot be mixed | 001 | Static |

## Identity and slots
| ID | Invariant | From | Check |
|---|---|---|---|
| S1 | `live_slots + free_slots + retired_slots == N` | 011 | State |
| S2 | A slot is live ⇔ its generation is odd ⇔ it is linked in exactly one level | 002, 011 | State |
| S3 | Free-list slots are even-generation and fully detached (`prev == next == NIL`) | 010 | State |
| S4 | An ID never refers to two different orders over the engine's lifetime | 002 | Stream |
| S5 | `size_of::<Slot>() == 32` | 011 | Static |

## Levels and book
| ID | Invariant | From | Check |
|---|---|---|---|
| L1 | `head == NIL ⇔ tail == NIL ⇔ total == 0 ⇔ occupancy bit clear` | 009 | State |
| L2 | `level.total == Σ remaining` over linked slots | 009 | State |
| L3 | Links are consistent: `next.prev == self`, `prev.next == self`, head has `prev == NIL`, tail has `next == NIL` | 010 | State |
| L4 | Every slot linked in level (side, p) has `side` and `price == p` | 009 | State |
| L5 | Within a level, queue order == arrival order (sequence of resting) | 010 | State |
| L6 | Book never crossed: `best_bid < best_ask` when both exist | 009 | State |

## Matching
| ID | Invariant | From | Check |
|---|---|---|---|
| M1 | `Trade.price` == the maker's resting price | 008 | Stream |
| M2 | Taker limit respected: buy trade price ≤ limit, sell ≥ limit | 004 | Stream |
| M3 | Price-time priority: each trade hits the best price, oldest order first | 010 | Stream (vs reference model) |
| M4 | Post-only never produces a trade on arrival | 003 | Stream |
| M5 | IOC/market orders never produce `Rested` | 004 | Stream |
| M6 | No trade has taker account == maker account | 006 | Stream |
| M7 | Makers touched per order ≤ qty + 1 | 006 | Stream |

## Event contract
| ID | Invariant | From | Check |
|---|---|---|---|
| E1 | Per New: Σ trade qty + terminal qty == order qty | 008 | Stream |
| E2 | Every order that becomes live gets exactly one terminal event | 008 | Stream |
| E3 | `MakerFilled{id}` immediately follows the trade with `maker_remaining == 0` | 008 | Stream |
| E4 | After its terminal event, an ID appears only in `Rejected` for late commands | 008 | Stream |
| E5 | `Rejected` ⇒ no state change (book identical before and after) | 008 | State |
| E6 | Same input sequence ⇒ identical event sequence | 012 | Stream (replay twice) |

## Ownership and modify
| ID | Invariant | From | Check |
|---|---|---|---|
| O1 | An order changes only via its own account's commands or by trading | 006 | Stream |
| O2 | Reduce never increases `filled + remaining` and never changes queue position | 007 | State |
| O3 | A rejected Replace leaves the old order untouched | 007 | State |

## Capacity
| ID | Invariant | From | Check |
|---|---|---|---|
| C1 | Allocations per command == 0 | 011 | Counting allocator test |
| C2 | GTC residual is cancelled for capacity only with reason `CapacityExhausted` | 011 | Stream (forced by tiny-capacity test) |
