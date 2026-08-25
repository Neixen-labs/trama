# trama-routing

Fastest paths over a TRAMA graph, costed by a speed column or by distance, honouring one-way
edges, and written into a state channel as a vehicle's progress.

It also plans a fleet: several vehicles from one depot, a load at every stop and a capacity per
van, by Clarke-Wright savings with 2-opt and a consolidation pass, measured against the true
optimum on instances small enough to enumerate it. Stops may carry time windows — arriving early
waits, arriving late is refused — and the plan is measured against the best *feasible* round
rather than the best one. The depot may carry one too: no van leaves before the yard opens, and
every round is home before it shuts, which is the only constraint here that prices the leg back.

Both honour turn restrictions when told which column holds them, settling an arc together with how
far along the forbidden runs it stands, because the cheapest way onto an edge is not always part
of the cheapest way past it. A file declaring no restrictions leaves that automaton with one
state, so the common case pays nothing.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
