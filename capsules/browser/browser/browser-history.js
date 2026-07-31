export function commitHistoryState({ entries, index }, url, mode) {
  if (mode === "none") {
    return { entries, index };
  }
  if (mode === "replace" && index >= 0) {
    const nextEntries = [...entries];
    nextEntries[index] = url;
    return { entries: nextEntries, index };
  }
  if (entries[index] === url) {
    return { entries, index };
  }
  const nextEntries = entries.slice(0, index + 1);
  nextEntries.push(url);
  return { entries: nextEntries, index: nextEntries.length - 1 };
}
