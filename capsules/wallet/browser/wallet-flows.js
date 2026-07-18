import { textNode } from "./wallet-render.js?v=wallet-20260718a";

export function createWalletFlows({ modalNode, modalBackdropNode, showStatus }) {
  function openInfoModal(title, message) {
    openFlowModal(title, message, [], [modalButton("Done", closeModal)]);
  }

  function openFlowModal(title, subtitle, nodes, actions = [modalButton("Cancel", closeModal, true)]) {
    modalNode.replaceChildren();
    const header = document.createElement("header");
    header.className = "wallet-modal-header";
    header.append(textNode("h2", title), textNode("p", subtitle, "wallet-state"));
    const body = document.createElement("div");
    body.className = "wallet-modal-body";
    for (const node of nodes) {
      body.append(node);
    }
    const footer = document.createElement("footer");
    footer.className = "wallet-modal-actions";
    for (const action of actions) {
      footer.append(action);
    }
    modalNode.append(header, body, footer);
    modalNode.hidden = false;
    modalBackdropNode.hidden = false;
  }

  function closeModal() {
    modalNode.hidden = true;
    modalBackdropNode.hidden = true;
  }

  function flowRow(title, subtitle, onClick) {
    const row = document.createElement("button");
    row.className = "wallet-flow-row";
    row.type = "button";
    row.append(textNode("div", "", ""));
    row.firstChild.append(textNode("strong", title), textNode("span", subtitle));
    row.append(textNode("span", "›", "wallet-state"));
    row.addEventListener("click", () => {
      Promise.resolve(onClick(row)).catch((error) => showStatus(String(error.message || error), "error"));
    });
    return row;
  }

  function flowStaticRow(title, value) {
    const row = document.createElement("div");
    row.className = "wallet-flow-row";
    row.append(textNode("strong", title), textNode("span", value));
    return row;
  }

  function modalButton(label, onClick, secondary = false, danger = false) {
    const button = document.createElement("button");
    button.className = [
      "wallet-button",
      secondary ? "wallet-button-secondary" : "",
      danger ? "wallet-button-danger" : "",
    ].filter(Boolean).join(" ");
    button.type = "button";
    button.textContent = label;
    button.addEventListener("click", () => {
      Promise.resolve(onClick(button)).catch((error) => showStatus(String(error.message || error), "error"));
    });
    return button;
  }

  return {
    closeModal,
    flowRow,
    flowStaticRow,
    modalButton,
    openFlowModal,
    openInfoModal,
  };
}
