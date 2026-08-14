# Capsules

A capsule is a signed package. It starts with nothing: no host files, no sockets, no credentials, no routes.

Roles:

- app. What a person opens. Public name is App.
- shell. Draws Runtime facts. Emits typed intents. Not a provider.
- viewer. Opens declared content.
- provider. A service other capsules use. Runtime must register it.
- content. Data. Optional viewer.

How it talks: Bus (`elastos:bus@v1`) for components. Web Apps are still projections under the same authority model.

It may not pick a transport, see a host path, hold a raw CEK, or treat a capability token as "I am this person."

Home is the trusted front door. `home-gui` and `home-cli` are shells. They have no Carrier, wallet, or provider authority of their own.
