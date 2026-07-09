# Cost is estimated list-price value, not billed spend

TokenLedger reports Cost as the public list-price value of the tokens consumed —
an estimate of what the usage *would* cost at pay-as-you-go API rates — not money
actually billed. Every Source is subscription, free-tier, or self-hosted, so
there is no per-token invoice to read; the alternatives were to show cost only
for truly metered usage (nothing qualifies here) or to show no dollar figure at
all.

We chose estimated value because it makes usage comparable across tools on one
scale and answers "what is this usage worth?", at the cost of a dollar figure
that must not be read as spend. The UI label ("Est. cost — at API list prices —
not billed") and the glossary definition of Cost carry that caveat.
