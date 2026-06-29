#lang racket

(define (sieve n)
  (define (mark multiples)
    (for ([m (in-list multiples)])
      (vector-set! is-prime m #f)))
  (define is-prime (make-vector (add1 n) #t))
  (vector-set! is-prime 0 #f)
  (vector-set! is-prime 1 #f)
  (for ([i (in-range 2 (add1 (sqrt n)))])
    (when (vector-ref is-prime i)
      (mark (range (* i i) (add1 n) i))))
  (for/list ([i (in-range 2 (add1 n))] #:when (vector-ref is-prime i)) i))

(printf "Primes up to 100: ~a\n" (sieve 100))
