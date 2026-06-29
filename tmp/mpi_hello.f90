program hello
  use mpi
  implicit none
  integer :: rank, size, ierr

  call MPI_INIT(ierr)
  call MPI_COMM_RANK(MPI_COMM_WORLD, rank, ierr)
  call MPI_COMM_SIZE(MPI_COMM_WORLD, size, ierr)
  print '(a,i0,a,i0)', 'Hello from rank ', rank, ' of ', size
  call MPI_FINALIZE(ierr)
end program hello
