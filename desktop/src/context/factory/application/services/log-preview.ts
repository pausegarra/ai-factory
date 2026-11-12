export function logPreview(text: string): { prefix: string; truncated: boolean } {
  let lineCut = text.length;
  let cursor = 0;
  for (let line = 1; line <= 12; line++) {
    const lf = text.indexOf("\n", cursor);
    if (lf < 0) break;
    if (line === 12 && lf + 1 < text.length) {
      lineCut = lf > 0 && text[lf - 1] === "\r" ? lf - 1 : lf;
      break;
    }
    cursor = lf + 1;
  }

  let charCut = 0;
  let points = 0;
  for (const point of text) {
    if (points === 1200) break;
    points++;
    charCut += point.length;
  }
  if (charCut > 0 && text[charCut - 1] === "\r" && text[charCut] === "\n") charCut--;

  const end = Math.min(lineCut, charCut);
  return { prefix: text.slice(0, end), truncated: end < text.length };
}
