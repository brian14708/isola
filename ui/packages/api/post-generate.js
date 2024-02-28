// walk ./src and prepend `//@ts-nocheck` to all files
import * as fs from "fs";
import * as path from "path";

function walk(dir, callback) {
  fs.readdir(dir, function (err, files) {
    if (err) throw err;
    files.forEach(function (file) {
      var filepath = path.join(dir, file);
      fs.stat(filepath, function (err, stats) {
        if (stats.isDirectory()) {
          walk(filepath, callback);
        } else if (stats.isFile()) {
          callback(filepath, stats);
        }
      });
    });
  });
}

walk("./src", (e) => {
  if (e.endsWith(".ts")) {
    fs.readFile(e, "utf8", function (err, data) {
      if (err) {
        return console.log(err);
      }
      if (!data.startsWith("// @ts-nocheck")) {
        fs.writeFile(e, "// @ts-nocheck\n" + data, "utf8", function (err) {
          if (err) return console.log(err);
        });
      }
    });
  }
});
