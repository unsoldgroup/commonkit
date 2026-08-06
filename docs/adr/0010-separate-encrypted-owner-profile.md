# Keep owner memory separate and encrypted

CommonKit stores the kit owner’s About Me Profile in a dedicated encrypted SQLite database rather than in Git or a project-memory database. This keeps personal deletion, access rules, writer authority, and portability independent from context-mode and Engram while reusing CommonKit’s encrypted snapshot model. Organization rules remain reviewable in Git; personal text never enters portable Git state.
