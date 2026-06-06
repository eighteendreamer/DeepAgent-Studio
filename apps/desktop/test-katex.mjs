import katex from 'katex';
import 'katex/contrib/mhchem/mhchem.js';
try {
  console.log(katex.renderToString('\\ce{H2O}'));
} catch (e) {
  console.error('Error:', e.message);
}
