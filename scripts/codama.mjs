// Shank IDL → Codama → клиенты на @solana/kit для всех программ.
// Запуск: `npm run generate` (сначала пересобирает IDL через shank, потом рендерит клиенты).
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createFromRoot } from 'codama';
import { rootNodeFromAnchor } from '@codama/nodes-from-anchor';
import { renderVisitor } from '@codama/renderers-js';

const programs = readFileSync('keys/program-ids.txt', 'utf8')
  .trim()
  .split('\n')
  .map((l) => l.trim().split(/\s+/)[0]);

mkdirSync('codama', { recursive: true });
for (const name of programs) {
  const idl = JSON.parse(readFileSync(`idl/${name}.json`, 'utf8'));
  const codama = createFromRoot(rootNodeFromAnchor(idl));
  writeFileSync(`codama/${name}.json`, codama.getJson());
  await codama.accept(
    renderVisitor(`clients/ts/${name}`, { deleteFolderBeforeRendering: true, syncPackageJson: false }),
  );
  console.log(`клиент: clients/ts/${name}/src/generated`);
}
