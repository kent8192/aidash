---
name: tenant-configuration-guide
description: Configure a Tenant, build missing capabilities and hand verified work to its agents.
---

# Help the person get the result they asked for

Reply in the language of the person's request, starting with your very first message. Describe the outcome in everyday words. A person asking for recurring work can tell you what they want without naming tools, writing code or designing the workflow. Ask only for decisions that change the outcome, such as who may publish, who receives the files, the schedule or the spending limit.

1. Read tenant_overview. Identify the Node (the team's workspace), existing agents, reusable definitions, connections and Duties. A Duty is recurring work. Reuse what already fits. The Marketplace is one source of ready-made capabilities. If it lacks what the person needs, build the missing part.
2. When you need the sandbox, use catalog_review_approve on your own exact entry and Node from the overview with node scope. One review covers the offered sandbox tools and their dependencies. A Catalog Approval permits a specific registered version. Approval Scope chooses where it can run. Check which tools are available after review. Explain any Deployment limit and continue with the parts you can do.
3. Use scratch_space_configure without arguments to read this Thread's Scratch Space and its current limits. Keep the space's id. Use only the hosts and External services the work requires. If a settings change is refused, read its details and retry with permitted settings if they still meet the request. Explain any access or limit a Deployment Administrator must change to do the remaining work. Build reusable scripts in the space, install the dependencies they need, run them on representative input and inspect the actual output. For authenticated APIs, use an approved External service and its OAuth Connection where supported. Keys and account consent come from the person through setting_request.
4. Turn the verified script into a callable tool for the agent, or bind the sandbox operation and attach a Skill giving the script's exact invocation, inputs and outputs. An Agent Plugin supplies packaged capabilities. A Skill supplies reusable instructions. Run the agent once with representative input to verify it can find and use what you prepared. For a new agent or Duty Thread, open it on the same Node with scratch_space_id set to the prepared space's id. Create the Duty in that Thread. Its Runs use the Thread's Scratch Space. Save deliverables into the Working Area and point the person to the resulting files. Installed dependencies and temporary files can stay in the Scratch Space.
5. After registry_create, agent_create, mcp_server_add, tool_add, skill_add, agent_plugin_add or marketplace_install, call catalog_review_approve on the exact root id and version. This approves eligible dependencies and installations together. For a plugin attached to an agent, review the agent. Models are Tenant-wide. Use catalog_approve for one version. Report blocked dependencies in the person's language and revisit them once the prerequisite is ready.
6. Keep the draft id and revision. Inspect, correct and reuse that draft if registration fails. Archive an abandoned draft you created after checking its status. If registration succeeded and approval failed, review the registered version. Continue independent steps after a failure. Report what ran, what the person can use and what is waiting for a decision or missing access.

## Carry the person's language into the work

In every agent definition and Duty instruction you write, tell the agent to reply in the requesting person's language from its first message, unless the person asks for another language. For recurring work that starts without a new message, record the language of the request that set it up so the agent knows which language to use in replies and reports.

## Show people what you made

When you make something the person should look at, show it in the dashboard. Save files in the Working Area and reference the saved file so the person can open it in File preview. For an interactive result, serve it through Live preview and give the person the link. Include these instructions in every agent definition you write.

## Ask for sensitive settings

Use setting_request for a Tenant Secret (a key available only to its named hosts), OAuth consent, payment, or a Tenant or Node purge. The person enters the value, connects the account, checks out or confirms the purge in the form. Keep secret values out of messages, scripts, tool arguments and logs. Payment checkout may be unavailable. Explain a pending purchase or unanswered form and skip the work that depends on it.

## Choose a Duty model within Budget

Multiply the average cost of the last seven comparable Runs by Runs per month. Without history, estimate each Run at 20,000 input and 2,000 output tokens, priced separately. Multiply output tokens by two for high reasoning effort, three for xhigh and four for max. Multiply the sum by Runs per month. Compare the unrounded estimate with 80% of the available Budget. Choose only a usable model within that amount. Give whole-dollar monthly estimates, using “under one dollar” in the person's language for smaller amounts. If pricing is unknown or no model fits, ask the person to choose the schedule, model or Budget through a choice form.

If the requested model is refused by the Tenant's Data Protection Level, explain the protection and what a change means for the team's content. Use data_protection_set to select the least restrictive level admitting the model within the Deployment floor. Keep the endpoint and Plan Connection switches unless the person asks to change them. Report the previous and effective levels and impact, then recheck the model. If the floor prevents the change, offer an eligible model.
