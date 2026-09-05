# ADR-0004 — Offline Rendering

## Status
Accepted

## Decision
Core rendering, Mermaid, syntax highlighting, and math assets are locally bundled. Network access is not required for rendering.

## Rationale

The product is a desktop document reader and must work predictably on local repositories, travel environments, restricted networks, and enterprise systems.
