const tabs = [...document.querySelectorAll('[role="tab"]')];
const panel = document.querySelector('#install-panel');
const guide = document.querySelector('#install-guide');
for (const tab of tabs) {
  tab.addEventListener('click', () => {
    for (const item of tabs) {
      item.setAttribute('aria-selected', String(item === tab));
      item.tabIndex = item === tab ? 0 : -1;
    }
    panel.setAttribute('aria-labelledby', tab.id);
    guide.href = `https://github.com/Elacity/elastos-runtime/tree/upstream/0.7.1-dev/docs/${tab.id === 'mac-arm' ? 'MAC' : 'INSTALL'}.md`;
  });
  tab.addEventListener('keydown', (event) => {
    const index = tabs.indexOf(tab);
    const next = { ArrowRight: (index + 1) % tabs.length, ArrowLeft: (index + tabs.length - 1) % tabs.length, Home: 0, End: tabs.length - 1 }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    tabs[next].focus();
    tabs[next].click();
  });
}

// Enable the control with the 0.7.1 installer deployment, after its served-byte proof.
const copy = document.querySelector('#copy-install');
copy.addEventListener('click', async () => {
  const status = document.querySelector('#copy-status');
  try {
    await navigator.clipboard.writeText(document.querySelector('#install-command').textContent);
    copy.textContent = 'Copied';
    status.textContent = 'Installation command copied.';
    setTimeout(() => { copy.textContent = 'Copy'; }, 2000);
  } catch {
    const range = document.createRange();
    range.selectNodeContents(document.querySelector('#install-command'));
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    status.textContent = 'Command selected. Use your browser’s Copy command.';
  }
});
