const fs = require('fs');
const path = require('path');
const dir = path.join(__dirname, 'src/components/settings');
const files = fs.readdirSync(dir);

files.forEach(file => {
  if (file !== 'GeneralSettings.tsx' && file.endsWith('Settings.tsx')) {
    const p = path.join(dir, file);
    let content = fs.readFileSync(p, 'utf8');
    content = content.replace(/<div className="w-full h-full bg-white overflow-y-auto px-16 pt-16 pb-20">/, '<>');
    content = content.replace(/<\/div>\s*\);\s*}/, '</>\n  );\n}');
    fs.writeFileSync(p, content);
  }
});
