.SUFFIXES: .hs .o
HC = ghc
HFLAGS = -O2 -Wall

all: main

main: Main.hs
	$(HC) $(HFLAGS) -o $@ $<

clean:
	rm -f main *.o *.hi
