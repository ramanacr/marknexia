# ADR-0003 — Dedicated Navigation Resolver

## Status
Accepted

## Decision
Implement a dedicated, testable URI/navigation resolver rather than relying on browser navigation semantics.

## Rationale

Local Markdown links have repository-relative semantics that differ from arbitrary web navigation. Cross-document fragments require application orchestration.

## Consequences

All Markdown links pass through one resolver. Outline clicks and heading anchor clicks reuse the same navigation pipeline.
