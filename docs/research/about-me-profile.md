# About Me profile research

## Key takeaways

- SQLite FTS5 provides local ranked full-text search. CommonKit keeps typed claims as the source of truth and treats the FTS table as rebuildable.
- SQLCipher provides transparent whole-file SQLite encryption, allowing the profile and search index to remain encrypted at rest.
- Engram demonstrates FTS5 search with personal and project scopes, but CommonKit deliberately gives personal memory its own database and review rules.
- MCP security guidance favors separate, least-privilege read and write capabilities for personal data.

## Sources

- SQLite FTS5: https://www.sqlite.org/fts5.html
- SQLCipher design and documentation: https://www.zetetic.net/sqlcipher/about/
- Engram technical reference: https://github.com/Gentleman-Programming/engram/blob/main/DOCS.md
- MCP authorization guidance: https://modelcontextprotocol.io/docs/tutorials/security/authorization
