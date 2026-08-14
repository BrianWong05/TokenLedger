# Issue tracker: Jira

Issues and PRDs for this repository live in Jira project **TokenLedger**
(key `TOKL`) on `https://pretenders.atlassian.net`
(cloudId `993f969d-48a4-4d20-9e74-bb26e069f87b`).

Use the Atlassian MCP tools for all operations. Issue types available in
`TOKL`: Epic, Story, Task, Bug, Sub-task.

## Conventions

- Create an issue with `createJiraIssue` (`projectKey: "TOKL"`).
- Read an issue and its discussion with `getJiraIssue` (pass `comment` in
  `fields` to include comments).
- List work with `searchJiraIssuesUsingJql`, e.g.
  `project = TOKL AND status != Done ORDER BY created DESC`.
- Update content or labels with `editJiraIssue`.
- Add discussion with `addCommentToJiraIssue`.
- Close completed or rejected work with `transitionJiraIssue` (find the
  transition id with `getTransitionsForJiraIssue`).

The workflow is Backlog → Selected for Development → In Progress → Done, and
new issues open in **Backlog**. Every transition is global, so any status can
be reached from any other in one call.

GitHub Issues on `BrianWong05/TokenLedger` are frozen: issues #1–#191 are
closed history. Do not open new ones.

## Pull requests as a triage surface

Pull requests are not a feature-request or triage surface for this repository.

## Skill instructions

When a skill says to publish to the issue tracker, create a Jira issue in
`TOKL`. When a skill says to fetch the relevant ticket, read the Jira issue and
its comments.
