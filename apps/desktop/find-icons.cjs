const fs = require('fs');
const path = require('path');

const icons = { fas: new Set(), far: new Set(), fab: new Set() };

function walk(dir) {
  const files = fs.readdirSync(dir);
  for (const file of files) {
    const p = path.join(dir, file);
    if (fs.statSync(p).isDirectory()) {
      walk(p);
    } else if (p.endsWith('.tsx') || p.endsWith('.ts')) {
      const content = fs.readFileSync(p, 'utf8');
      const regex = /icon=\{\[\"(fas|far|fab)\",\s*\"([a-z0-9\-]+)\"\]\}/g;
      let match;
      while ((match = regex.exec(content)) !== null) {
        icons[match[1]].add(match[2]);
      }
      // also check icon: ["fas", "something"]
      const regex2 = /icon:\s*\[\"(fas|far|fab)\",\s*\"([a-z0-9\-]+)\"\]/g;
      let match2;
      while ((match2 = regex2.exec(content)) !== null) {
        icons[match2[1]].add(match2[2]);
      }
      
      // also check iconName: ["fas", "something"]
      const regex3 = /iconName:\s*\[\"(fas|far|fab)\",\s*\"([a-z0-9\-]+)\"\]/g;
      let match3;
      while ((match3 = regex3.exec(content)) !== null) {
        icons[match3[1]].add(match3[2]);
      }
    }
  }
}

walk(path.join(__dirname, 'src'));

console.log('fas:', [...icons.fas].join(', '));
console.log('far:', [...icons.far].join(', '));
console.log('fab:', [...icons.fab].join(', '));
