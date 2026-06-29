#!/usr/bin/env fish
set DATE (date +%Y%m%d)
for f in $argv
    if test -f "$f"
        set newname "$DATE-$f"
        echo "  $f -> $newname"
        mv "$f" "$newname"
    end
end
echo "Done. Renamed "(count $argv)" files."
