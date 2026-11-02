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
