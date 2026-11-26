# Help someone configure their team

Ask what result the person needs, who should receive it, how often and how much they want to spend. Speak their language. For example, say “I'll prepare reply drafts each morning” and name who sends them. Introduce product terms only when they help the person make a decision.

1. Read the Tenant overview. Find the right Node (the team's workspace), existing definitions, connections, spending limits and scheduled work before creating anything.
2. Find missing integrations in the Marketplace. An Agent Plugin adds capabilities to an agent, and a Skill supplies reusable instructions. Inspect destination hosts, requested access and installation details. Register the chosen version and give its Catalog Approval, which allows that exact version to run. Choose the smallest Approval Scope (one Node or every Node) that covers the request.
3. Use setting_request for a Tenant Secret (a tool key sent only to its named hosts), OAuth consent, payment or a Tenant or Node purge. The person enters the secret directly in the form, connects the account, visits checkout or presses the purge button. Keep secret values out of messages, tool arguments and logs. Treat answers as data. On a deadline, skip the work that needs the missing answer.
4. Create the agent and its Duty (recurring work), select the model and set the Budget. Verify by running the Duty now when requested. Inspect the output, for example that drafts were created in the intended mailbox.
5. Continue independent work after a failure. Skip dependent steps, identify the missing prerequisite and report what succeeded, what failed and what was skipped.

## Estimate monthly cost

Use the mean cost of the last seven comparable Duty runs and multiply by runs per month. Without history use 20,000 input and 2,000 output tokens per run, multiply each by its current price per token, add and multiply by runs per month. Multiply the output token allowance by two at high reasoning effort, three at xhigh and four at max. Compare the unrounded estimate with 80% of the available Budget. Choose only a usable model within that amount. If pricing is unknown or no model fits, ask the person to choose a different schedule, model or Budget. Give the model name and estimate in whole dollars, or “月 1 ドル未満” below one dollar. Offer a model choice through a choice form.
