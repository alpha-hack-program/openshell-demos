Review README.md at root. The idea of having one base demo to build on top is not the way we did it. Instead we have a base demo with a smoke test then a SPIRE demo thatś totally unrelated. Fix the README.md to reflect the reality.

Fix base demo README.md to use the same approach as in demos/spire with regards to creating a claude sandbox with anthrpoic api.

In the demos/spire README.md is explained the step to generate the refresh token and the reason to do it?

Add a brief explanation the demo flow and use a mermaid diagrams for it because there's an admin flow and a customer flow (one per customer, 1 and 2 actually)

I'd like to rename the demos/spire demo with a proper name we don't use spire, suggest some names, then apply the select name to the guide and the name of the openshift project.

# Priority 1

## Demo Base

- [x] Smoke test should be done with Code + OpenAI API KEY

## Demo Kaycloak OIDC

- [x] Script 01-deploy-keycloak.sh should print the values to copy into demo specific .env
- [x] I want the code of this script ./scripts/02-apply-oidc-overlay.sh used directly not hidden this is a guide to learn
- [x] The demo README.md should explain that .env needs to be filled in and how
- [x] In the guide if there are two option you can choose from give a short summary before those options, oehterwise you have to read through to now what's de difference
- [x] Option B — Browser-based authorization code flow (closer to production) is superceeded by Option C, should be deleted
- [x] Add a github option that compiles util/onboard for Linux and Macos
- [x] In Step 2 there has to be a code section with the code that is run by onboard util and avoid the script, this guide is to learn and when referring to onboard util running those commands point to the code section above (when you have already added it)
- [x] The step "Before running this, substitute the real Keycloak host into providers/customer-refresh-profile.yaml'" is not well explained, I don't know what to do with this, and people running the guide without a monitor won't understand either, reword.
- [x] We don't want demo.sh I prefer commands to copy and paste and explained
- [x] For clarity move roubleshooting to a troubleshooting section to avoid clutter a guide that should follow the happy path
- [x] We should talk about users more than customers the idea is that we provide an infra so that different users with different roles can do different things for the job they have to carry out.
- [x] The demo is the important part, there should be a summary of what we're going to do and value it adds. Additionally there has to be a section in the demo summary or after it that explains the RBAC setup and the role the admin and customers play.
- [x] Rename the demos/spire demo with a proper name we don't use spire after all, suggest some names, then apply the select name to the guide and the name of the openshift project.
