#!/usr/bin/env perl
use strict;
use warnings;
use List::Util qw(sum min max);

# Word frequency counter
my %freq;
while (<>) {
    $freq{lc $_}++ for /(\w+)/g;
}

my @sorted = sort { $freq{$b} <=> $freq{$a} } keys %freq;
for my $word (@sorted[0..9]) {
    printf "%-20s %d\n", $word, $freq{$word};
}
