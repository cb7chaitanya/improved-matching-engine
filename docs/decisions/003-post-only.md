# 003 — Post-only

## Decision
- A post-only order **rests without a fill or is rejected**. It is never repriced ("slide").
  Rationale: preserves the submitted price, one crisp rule, the maker's software decides what next.
- Marketable test (equality counts as crossing):
  buy: `price >= best_ask`; sell: `price <= best_bid`. Empty opposite side → not marketable → rests.
- Order kinds are an enum; invalid combinations cannot be constructed in the core:
  ```rust
  enum OrderKind {
      Limit    { price: Price, tif: Tif },     // Tif = Gtc | Ioc
      Market   { protection: Option<Price> },
      PostOnly { price: Price },              // no tif: always rests or is rejected
  }
  ```
  The gateway parses wire flags into this enum and rejects contradictions
  (post-only + IOC, post-only + market). "Parse, don't validate."

## Rejection precedence (deterministic; same input → same reason)
1. Static validation: `InvalidPrice`, `InvalidQty`
2. `PostOnlyWouldCross`
3. `BookFull`

Checks happen **before** slot allocation: rejections consume no generation and need no free path.
